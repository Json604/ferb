import AppKit
import Combine
import SwiftUI
import Quartz
import NotchCore
import os

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
    /// Command-C, X, A, V, Z and Shift-Command-Z go to the focused view's responder chain directly: text selected in
    /// the chat can be copied (owner, 2026-10-24: "i cant copy by dragging") and every field takes paste, undo and redo
    /// (owner, 2026-11-05: "no pasting in ferb question input boxes -fix"; APP-717). The composer's own paste (paste
    /// provenance, marker clean-up) is what `paste:` runs there, once. A hidden view gets nothing (APP-558).
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if let action = Self.editAction(for: event), let responder = firstResponder,
           (responder as? NSView)?.isHiddenOrHasHiddenAncestor != true,
           Self.canPerform(action, undo: responder.undoManager),
           responder.tryToPerform(action, with: self) { return true }
        return super.performKeyEquivalent(with: event)
    }

    /// Undo and redo run only with something to undo or redo; the window's undo manager is never asked otherwise.
    static func canPerform(_ action: Selector, undo manager: UndoManager?) -> Bool {
        switch action {
        case Selector(("undo:")): manager?.canUndo == true
        case Selector(("redo:")): manager?.canRedo == true
        default: true
        }
    }

    static func editAction(for event: NSEvent) -> Selector? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask).subtracting([.capsLock, .numericPad])
        let key = event.charactersIgnoringModifiers?.lowercased()
        if flags == [.command, .shift] { return key == "z" ? Selector(("redo:")) : nil }
        guard flags == .command else { return nil }
        return switch key {
        case "c": #selector(NSText.copy(_:))
        case "x": #selector(NSText.cut(_:))
        case "a": #selector(NSResponder.selectAll(_:))
        case "v": #selector(NSText.paste(_:))
        case "z": Selector(("undo:"))
        default: nil
        }
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
    /// The owner's own shortcut to open FERB (D24), recorded in Settings → Keyboard & voice.
    let hotKeys: HotKeyCenter
    /// The owner's controls as they happen, for the intro's Control beat (IntroController).
    let introControls = IntroControlSignals()
    private let overlaySpace: NotchOverlaySpace
    /// Space changes, screen-parameter notices, wake, presentations, yields and hover, at notice level so the system
    /// log keeps them (APP-729): states, frames and window numbers only, never chat text.
    private static let spacesLog = Logger(subsystem: "com.ferb.FERB", category: "spaces")
    private static let overlayBehavior: NSWindow.CollectionBehavior = [
        .canJoinAllSpaces, .canJoinAllApplications, .stationary,
        .ignoresCycle, .fullScreenAuxiliary, .fullScreenDisallowsTiling
    ]
    private var presentationRevision = UUID()
    private var presentedBodyVisible = false
    private let panel: NotchPanel
    private let tracerPanel = NotchLightPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                                              backing: .buffered, defer: false)
    let indicatorPanel = FerbIndicatorPanel(contentRect: CGRect(x: 0, y: 0, width: 58, height: 30),
                                                    styleMask: [.borderless, .nonactivatingPanel],
                                                    backing: .buffered, defer: false)
    /// The camera housing's topmost point row while docked, reaching above the screen's top edge (APP-710).
    let edgeRowPanel = FerbIndicatorPanel(contentRect: CGRect(x: 0, y: 0, width: 58, height: 2),
                                          styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    private var activityPanel: TaskActivityPanel?
    private var activityTaskID: String?
    private var geometry: NotchGeometry?
    private var statusItem: NSStatusItem?
    /// The pending hover open or hover-out fold (tests await it).
    private(set) var hoverTask: Task<Void, Never>?
    /// Where the pointer is, in screen coordinates (tests place it).
    var pointerLocation: () -> CGPoint = { NSEvent.mouseLocation }
    /// How the hover open and hover-out grace pass (tests end them at once instead of racing a clock).
    var hoverWait: @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    /// How the edge pill's three idle seconds before it rests pass (tests end them at once).
    var restWait: @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    /// What a display reports about a camera housing: its top safe-area inset and the areas beside it (tests stand in
    /// a notchless display).
    var cameraHousing: (NSScreen) -> (top: CGFloat, left: CGRect?, right: CGRect?) = {
        ($0.safeAreaInsets.top, $0.auxiliaryTopLeftArea, $0.auxiliaryTopRightArea)
    }
    /// Whether a full-screen app's Space shows on a display (D14: the edge pill stays its resting line there).
    var fullScreenSpace: (UInt32) -> Bool = { WindowServerSpaces.isFullScreen(display: $0) }
    /// The Space showing now, as the window server says (an activation that comes with a Space change).
    var activeSpace: () -> UInt64? = { WindowServerSpaces.activeSpace() }
    /// Mission Control (or App Exposé) shows on a display (window-server coordinates): the pointer crossing FERB on its
    /// way to the Spaces bar opens nothing.
    var missionControlShowing: (CGRect) -> Bool = { display in
        SpaceFacts.missionControlShowing(in: ScreenWindow.onScreen(), display: display) { pid in
            NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "com.apple.WindowManager"
        }
    }
    /// How many times FERB laid out its presentation (tests check that a Space change or an unchanged screen notice
    /// doesn't).
    private(set) var presentCount = 0
    /// The Space FERB last saw become active.
    private var lastSpace: UInt64?
    /// After a Space change, hover waits for the pointer to really move: where it was at the change (APP-729).
    private var pointerAtSpaceChange: CGPoint?
    /// The screens as FERB last laid itself out on them; a notice that changes none of it changes nothing.
    private var appliedLayout: ScreenLayout?
    private var menuBarHeights = MenuBarHeights()
    /// The edge pill sits as its resting line.
    private(set) var pillResting = false
    private(set) var indicatorRestTask: Task<Void, Never>?
    private(set) weak var indicatorControl: FerbIndicatorControl?
    private var presentedExpanded = false
    private var isDocked = false
    /// Another app's window holds the hardware notch (a notch app): FERB rests beside it instead (APP-644).
    private var notchTaken: Bool { notchHolder != nil }
    /// What those windows cover (AppKit coordinates), which the handle steps aside from.
    private var notchHolder: CGRect?
    private var notchTakenRecheck: [DispatchWorkItem] = []
    private var dockingRevision: UUID?
    let magneticFieldPanel = NotchLightPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    /// The closed notch's timer live activity (APP-762): click-through, beside the camera housing or the pill.
    let timerActivityPanel = NotchLightPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                                             backing: .buffered, defer: false)
    private var timerActivityGap: CGFloat = -1
    private var timerActivityTick: Task<Void, Never>?
    private var timerActivityQueued = false
    /// A running timer held the pill up at the last look (APP-766): when it no longer does, the pill rests by itself.
    private var timerHeldPill = false
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
    private var indicatorEdge: FERBScreenEdge = .top
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
    /// The sticky window (APP-745), created the first time window mode shows the chat; never in notch mode.
    private(set) var chatWindow: ChatWindowController?
    /// A question, an answer or a reminder just arrived: the window comes in front of other windows (D22).
    private var arrivalPending = false
    /// Hide the pill as last applied (D21): the handle, edge row and pill windows stay out.
    private(set) var pillHidden = false
    private var defaultsObserver: NSObjectProtocol?
    private var statusRefreshQueued = false
    private var isStarted = false

    let cursor = CursorController()

    /// Where the notch's place is kept (`notch.*`: docked, edge, fraction, display).
    private let defaults: UserDefaults

    /// `model`, `defaults` and `overlaySpace` are for tests (a scripted daemon, a scratch suite, the AppKit fallback);
    /// the app uses its own, the standard defaults and SkyLight's overlay space.
    init(model: NotchModel? = nil, defaults: UserDefaults = .standard,
         overlaySpace: NotchOverlaySpace? = nil, hotKeyRegistrar: HotKeyRegistering? = nil) {
        let model = model ?? NotchModel()
        hotKeys = HotKeyCenter(defaults: defaults, registrar: hotKeyRegistrar)
        self.model = model
        self.defaults = defaults
        self.overlaySpace = overlaySpace ?? NotchOverlaySpace()
        panel = NotchPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                           backing: .buffered, defer: false)
        yieldController = NotchYieldController(model: model)
        windowYield = NotchWindowYield(panel: panel, tracer: tracerPanel, rejoinOverlay: { [space = self.overlaySpace] window in
            if let window = window as? NSWindow { space.rejoin(window) }
        }, leaveOverlay: { [space = self.overlaySpace] window in
            if let window = window as? NSWindow { space.leave(window) }
        })
        super.init()
        model.cursor = cursor
        // FERB's click-through windows, told to the daemon so its click check skips exactly them (D31 fix round 4)
        cursor.onPanelsChange = { [weak self] numbers in self?.model.clickThrough.set("cursor", windows: numbers) }
        yieldController.layout = { [weak self] in self?.yieldLayout() }
        configurePanel()
        configureIndicator()
        model.onPresentationChange = { [weak self] animate in self?.present(animate: animate, caller: "model") }
        model.onAttentionArrival = { [weak self] in
            self?.hoverTask?.cancel()
            self?.arrivalPending = true
        }
        model.onTourChange = { [weak self] in self?.applyChatHost() }
        model.onBackgroundChange = { [weak self] animate in
            self?.present(animate: animate, requestFocus: false, caller: "background")
        }
        // After a Quit & Reopen FERB itself is frontmost, so Settings never "comes forward": the tour asks (APP-642).
        model.onStepBehindSettings = { [weak self] in
            guard let self, let settings = NSRunningApplication.runningApplications(
                withBundleIdentifier: SettingsAnchor.bundleID).first else { return }
            if self.yieldController.appActivated(settings.processIdentifier) { self.updatePointer() }
            self.restackYield()
        }
        // A grant macOS confirmed: back above Settings to say so, keyboard focus left where the owner has it (APP-687).
        model.onComeForward = { [weak self] in
            guard let self else { return }
            self.windowYield.ownerMovedElsewhere()        // coming back must not make the panel key
            if self.yieldController.summon() { self.updatePointer() }
        }
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
        hotKeys.onPressed = { [weak self] in self?.toggleByShortcut() }
        // A file drag over the handle or pill (D17): the pill un-rests at once; held there, the chat opens by the
        // press path whatever the hover setting; a chat it opened folds when the drag leaves with nothing dropped.
        model.drops.onReach = { [weak self] in
            guard let self else { return }
            self.indicatorRestTask?.cancel()
            self.indicatorRestTask = nil
            self.setIndicatorResting(false, cause: "file drag")
        }
        model.drops.onSpring = { [weak self] in self?.openForFileDrag() }
        model.drops.pointerOver = { [weak self] in self?.pointerIsOverSurface ?? false }
        model.drops.onFold = { [weak self] in
            guard let self, self.model.expanded else { return }
            Self.spacesLog.notice("A file drag left FERB with nothing dropped: the chat it opened folds")
            self.model.collapse()
        }
        model.onYieldChange = { [weak self] yielded in self?.applyYield(yielded) }
        // Where FERB opens, Keep on top and Hide the pill take effect as Settings changes them (APP-745).
        defaultsObserver = NotificationCenter.default.addObserver(forName: UserDefaults.didChangeNotification,
                                                                  object: defaults, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.applyChatHost() }
        }
        applyChatHost()
        model.onOpenDocument = { [weak self] url in
            if HandOff.open(url) { self?.model.dismissAutomatically() }
            else {
                let alert = NSAlert()
                alert.messageText = "The artifact couldn’t be opened."
                alert.informativeText = "Try Quick Look, or reveal the file in Finder."
                alert.runModal()
            }
        }
        model.onQuickLook = { [weak self] url in self?.quickLook(url) }
        model.onSettings = { [weak self] in self?.showSettings() }
        model.onOpenWindow = { [weak self] in self?.onOpenWindow?() }
        model.onStatusChange = { [weak self] in
            self?.updateTracerVisibility()
            self?.updateYieldWatch()
        }
    }

    func start() {
        isStarted = true
        model.connectDaemon()
        configureMenu()
        poster?.onReminder = { [weak self] item in
            if case .reminder(let id) = item.target { self?.model.receiveReminder(id: id, text: item.body, card: item.card) }
        }
        poster?.start(rpc: model.rpc, notchShowing: { [weak self] in self?.notchShowing ?? false },
                      open: { [weak self] target in self?.open(target) })
        model.onApprovalResolved = { [weak self] id in self?.poster?.withdrawQuestion(id) }
        statusRefresh = model.objectWillChange.sink { [weak self] _ in self?.queueStatusRefresh() }
        shortcuts.onDoubleTap = { [weak self] in self?.modifierDoubleTapped() }
        shortcuts.doubleTapEnabled = { [defaults] in AppSettings.doubleTap.read(defaults) == "on" }
        KeyCombo.layoutCharacter = KeyboardLayout.character
        hotKeys.start()
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
            // The notch's panel or the sticky window, whichever holds the chat and has keyboard focus.
            guard let self, self.model.bodyVisible, let window = event.window, window.isKeyWindow,
                  window === (self.model.chatInWindow ? self.chatWindow?.panel : self.panel) else { return event }
            if self.model.session.phase == .result, !self.model.showingHistory, !self.model.showingSources,
               !self.model.showingActivity, !(window.firstResponder is NSTextView) {
                if event.keyCode == 49 { self.model.quickLook(); return nil }
                if event.keyCode == 36 || event.keyCode == 76 { self.model.openDocument(); return nil }
                if event.keyCode == 15 && event.modifierFlags.contains(.command) { self.model.revealDocument(); return nil }
            }
            if window.firstResponder is ChatTextView { return event }
            // Return in a field on FERB's intro line submits that field (its own onSubmit), never the composer.
            if self.model.introLine?.entry != nil, (window.firstResponder as? NSTextView)?.isFieldEditor == true {
                return event
            }
            guard self.model.canCompose,
                  event.keyCode == 36 || event.keyCode == 76,
                  let editor = window.firstResponder as? NSTextView,
                  !editor.hasMarkedText() else { return event }
            if event.modifierFlags.contains(.shift) {
                editor.insertNewlineIgnoringFieldEditor(nil)
            } else {
                self.model.startWork()
            }
            return nil
        }) { eventMonitors.append(monitor) }
        updateScreen()
        attachOverlay()
        observeWorkspace()
        if let monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] _ in
            self?.ownerClickedInAnotherApp()
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
        if let monitor = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged], handler: { [weak self] event in
            if event.type == .leftMouseDragged { self?.noticeFileDrag() }
            self?.updatePointer()
        }) { eventMonitors.append(monitor) }
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged], handler: { [weak self] event in
            if event.type == .leftMouseDragged { self?.noticeFileDrag() }
            self?.updatePointer()
            return event
        }) { eventMonitors.append(monitor) }
        dragPasteboardCount = dragPasteboard().changeCount
    }

    /// FERB's notch windows join the fixed overlay space (APP-563), with AppKit's all-Spaces windows as the fallback.
    func attachOverlay() {
        magneticFieldPanel.backgroundColor = .clear
        magneticFieldPanel.isOpaque = false
        magneticFieldPanel.hasShadow = false
        magneticFieldPanel.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 2)
        magneticFieldPanel.collectionBehavior = Self.overlayBehavior
        magneticFieldPanel.contentView = magneticField
        overlaySpace.attach([panel, tracerPanel, indicatorPanel, edgeRowPanel, magneticFieldPanel])
        if overlaySpace.isAttached, let chatWindow { chatWindow.space = overlaySpace }
        lastSpace = activeSpace()
        Self.spacesLog.notice("""
            Overlay attached \(self.overlaySpace.isAttached, privacy: .public) on Space \(self.lastSpace ?? 0, privacy: .public): \
            \(self.overlaySpace.describe(), privacy: .public)
            """)
    }

    /// Where one of FERB's windows is: in the overlay space, and on which ordinary Spaces (nil without the space).
    func overlayMembership(of window: NSWindow) -> (member: Bool, ordinary: [UInt64])? {
        overlaySpace.membership(of: window)
    }

    /// The display, wake, Space, activation and app launch notices FERB follows.
    func observeWorkspace() {
        isStarted = true
        NotificationCenter.default.addObserver(self, selector: #selector(displayParametersChanged),
                                              name: NSApplication.didChangeScreenParametersNotification, object: nil)
        let workspace = NSWorkspace.shared.notificationCenter
        workspace.addObserver(self, selector: #selector(screenDidWake), name: NSWorkspace.screensDidWakeNotification,
                              object: nil)
        workspace.addObserver(self, selector: #selector(activeSpaceChanged),
                              name: NSWorkspace.activeSpaceDidChangeNotification, object: nil)
        workspace.addObserver(self, selector: #selector(applicationActivated(_:)),
                              name: NSWorkspace.didActivateApplicationNotification, object: nil)
        workspace.addObserver(self, selector: #selector(screensDidSleep), name: NSWorkspace.screensDidSleepNotification,
                              object: nil)
        for name in [NSWorkspace.didLaunchApplicationNotification, NSWorkspace.didTerminateApplicationNotification] {
            workspace.addObserver(self, selector: #selector(applicationsChanged), name: name, object: nil)
        }
    }

    /// Orders one of FERB's windows in and keeps it in the overlay space, on no ordinary Space (APP-729).
    private func orderInOverlay(_ window: NSWindow) {
        // Hidden pill (D21): the handle, edge row and pill never show, whatever asks for them.
        if pillHidden, window === indicatorPanel || window === edgeRowPanel {
            window.orderOut(nil)
            return
        }
        window.orderFrontRegardless()
        overlaySpace.ensure(window)
    }

    /// The notch's geometry on a screen, from facts that a Space change doesn't touch: the camera housing, and the
    /// menu bar's height as last seen on that display (a full-screen Space or an auto-hiding menu bar reports none).
    private func notchGeometry(for screen: NSScreen) -> NotchGeometry {
        let housing = cameraHousing(screen)
        return NotchGeometry(screen: screen.frame, topInset: housing.top, leftAreaMaxX: housing.left?.maxX,
                             rightAreaMinX: housing.right?.minX, menuBarHeight: steadyMenuBarHeight(on: screen))
    }

    private func steadyMenuBarHeight(on screen: NSScreen) -> CGFloat {
        menuBarHeights.height(display: displayID(screen), visibleGap: screen.frame.maxY - screen.visibleFrame.maxY)
    }

    /// What FERB's layout depends on, now (see ScreenLayout).
    private func currentLayout() -> ScreenLayout {
        ScreenLayout(displays: NSScreen.screens.map { screen in
            let housing = cameraHousing(screen)
            return ScreenLayout.Display(id: displayID(screen), frame: screen.frame, topInset: housing.top,
                                        leftArea: housing.left, rightArea: housing.right,
                                        scale: screen.backingScaleFactor)
        })
    }

    /// One line of FERB's state for the `spaces` log (no text from the chat).
    private var stateLine: String {
        "expanded \(model.expanded) body \(model.bodyVisible) byPress \(model.staysOpenOnHoverOut) "
            + "yielded \(model.isYielded) \(model.yielding.reasons.map { "\($0)" }) panel \(panel.frame) visible \(panel.isVisible) "
            + "key \(panel.isKeyWindow) docked \(isDocked) resting \(pillResting) pointerInside \(pointerIsInside)"
    }

    /// A click that went to another app or the desktop (the global monitor). It closes the chat, a press-opened one
    /// included (APP-707).
    func ownerClickedInAnotherApp() {
        let location = pointerLocation()
        chatWindow?.ownerClicked(at: windowServerPoint(location))
        let cameBack = yieldController.ownerClicked(at: windowServerPoint(location))
        // The click went to another app: coming back must not take the focus the owner moved there.
        windowYield.ownerMovedElsewhere()
        if cameBack {
            // A click on the notch brought FERB back: focus the visible input, like hover and the shortcut.
            updatePointer()
            focusVisibleInput()
        }
        restackYield()
        guard model.expanded, !panel.frame.contains(location) else { return }
        model.dismissAutomatically()
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
        defer { configureTimerActivity() }
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
            self.indicatorDragPointer = point
            self.queueTimerActivity()
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
            self.model.toggle(byPress: true)
        }
        indicatorPanel.contentView = control

        edgeRowPanel.title = "FERB notch edge"
        edgeRowPanel.backgroundColor = .clear
        edgeRowPanel.isOpaque = false
        edgeRowPanel.hasShadow = false
        edgeRowPanel.hidesOnDeactivate = false
        edgeRowPanel.canBecomeVisibleWithoutLogin = false
        edgeRowPanel.isReleasedWhenClosed = false
        edgeRowPanel.level = indicatorPanel.level
        edgeRowPanel.collectionBehavior = Self.overlayBehavior
        edgeRowPanel.animationBehavior = .none
        edgeRowPanel.acceptsMouseMovedEvents = true
        let edgeRow = NotchEdgeRowView(frame: .zero)
        edgeRow.handle = control
        edgeRowPanel.contentView = edgeRow
    }

    private func showTaskActivity(_ taskID: String) {
        guard model.taskItems.contains(where: { $0.id == taskID && $0.isRunning }) else { return }
        if activityTaskID == taskID, let activityPanel {
            orderInOverlay(activityPanel)
            return
        }
        closeTaskActivity()
        let window = TaskActivityPanel(model: model, taskID: taskID) { [weak self] in self?.closeTaskActivity() }
        activityTaskID = taskID
        activityPanel = window
        overlaySpace.include(window)
        orderInOverlay(window)
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

    /// The handle is drawn inside FERB's shown surface (the chat, a tour line, a plan preview; `present` attaches it
    /// whenever the surface shows): a drag carries that surface along. Otherwise the handle has its own window.
    /// (Branching on the open chat alone moved an empty, hidden handle window during the tour: owner's live run,
    /// 2026-11-01, "not movable".)
    private var handleOnSurface: Bool { indicatorControl?.isAttached == true }

    private func moveIndicator(with point: CGPoint) {
        guard let origin = indicatorDragPointer, let start = indicatorDragFrame else { return }
        if isDocked {
            // The first real drag tears out; a click still just opens/closes chat.
            isDocked = false
            model.isNotchDocked = false
            presentation.backdrop.docked = false
            NotchDocking.save(false, in: defaults)
            indicatorControl?.isDocked = false
            indicatorEdge = .top
            model.indicatorEdge = "top"
            indicatorFrame = CGRect(x: origin.x - 29, y: origin.y - 15, width: 58, height: 30)
            indicatorDragFrame = indicatorFrame
            if handleOnSurface {
                // The pill emerges under the grabbed point, including a grab farther
                // down the empty header. Subsequent motion translates one window.
                let old = panel.frame
                panel.setFrame(CGRect(x: old.minX, y: origin.y + 15 - old.height - 28,
                                      width: old.width, height: old.height + 28), display: true)
                presentation.destination = CGPoint(x: 8, y: 28)
                presentation.present(size: model.surfaceSize, expanded: true,
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
        // The handle leaves the top edge's row behind while it moves; the drop lays it out again.
        if edgeRowPanel.isVisible { edgeRowPanel.orderOut(nil) }
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
        if handleOnSurface {
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
                pill: indicatorFrame, card: card, canvas: screen.frame)
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
        if dragCanvasActive, let control = indicatorControl, handleOnSurface {
            let card = CGRect(x: panel.frame.minX + presentation.surfaceOrigin.x,
                y: panel.frame.maxY - presentation.surfaceOrigin.y - presentation.size.height,
                width: presentation.size.width, height: presentation.size.height)
            DragSurfaceCanvas.place(window: panel, presentation: presentation, control: control,
                pill: indicatorFrame, card: card, canvas: card.union(indicatorFrame))
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
        let targetGeometry = notchGeometry(for: screen)
        if NotchDocking.accepts(point, cutout: targetGeometry.cutoutFrame,
                                hasHardwareNotch: targetGeometry.hasNotch && holder(of: targetGeometry) == nil) {
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
        queueTimerActivity()
    }

    private func startMagneticClock(on screen: NSScreen) {
        guard magneticClock == nil else { return }
        let clock = screen.displayLink(target: self, selector: #selector(tickMagnetic(_:)))
        magneticClock = clock
        clock.add(to: .main, forMode: .common)
    }

    private func sampleMagneticDrag(at time: TimeInterval) {
        guard let screen = indicatorScreen else { return }
        let notch = notchGeometry(for: screen)
        updateMagneticDrag(pill: magneticRawPill, target: notch.hasNotch && !notchTaken ? notch.cutoutFrame : nil,
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
            if handleOnSurface {
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
        if handleOnSurface {
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
        beginMagneticFusion(to: handleOnSurface ? NotchDocking.dragRegion(cutout: cutout) : cutout,
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
                    pill: indicatorFrame, card: card, canvas: fusionCanvas)
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
        if magneticFieldPresentationEnabled && !magneticFieldPanel.isVisible { orderInOverlay(magneticFieldPanel) }
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
        queueTimerActivity()
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
        NotchDocking.save(true, in: defaults)
        if let screen = indicatorScreen { persistIndicatorAnchor(for: screen) }
        fusionCardStart = nil
        fusionCardTarget = nil
        // updateScreen calls present without animation. No transparent reset frame,
        // secondary entrance spring, layer transform, or independent window fade.
        updateScreen()
        presentation.backdrop.magneticJoin = nil
        updatePointer()
        queueTimerActivity()
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
        let notch = notchGeometry(for: screen)
        // A notch app's window over the camera is stepped around whole, so the handle stays reachable (APP-644).
        let avoid = notch.hasNotch ? notchHolder.map { $0.union(notch.cutoutFrame) } ?? notch.cutoutFrame : nil
        return IndicatorPlacement.safeTopX(proposedX: proposedX, size: size.width, screen: bounds, notch: avoid)
    }

    /// macOS posts screen-parameter notices for more than display changes (a Space switch, a full-screen app, the
    /// menu bar or Dock hiding). Only a notice that changes what FERB's layout depends on (ScreenLayout) lays it out
    /// again; any other leaves the chat, the pointer state, the pill and a drag exactly as they are (APP-729).
    @objc func displayParametersChanged() {
        let layout = currentLayout()
        let changed = layout != appliedLayout
        Self.spacesLog.notice("""
            Screen parameters changed: layout \(changed ? "changed" : "unchanged", privacy: .public) \
            (\(layout.displays.map { "\($0.id ?? 0) \($0.frame) top \($0.topInset) scale \($0.scale)" }, privacy: .public)); \
            \(self.stateLine, privacy: .public)
            """)
        guard changed else { return }
        chatWindow?.screensChanged()
        cancelMagneticInteraction()
        if isStarted { updateScreen() }
    }

    /// Lays FERB out on its screen: docked in the hardware notch unless the owner tore it out (a first run docks), else
    /// on the edge where the owner dropped it (the top centre by default).
    @objc func updateScreen() {
        appliedLayout = currentLayout()
        model.acknowledgement.dismissTransient()
        let savedDisplay = defaults.object(forKey: "notch.indicatorDisplayID") as? NSNumber
        let preferred = savedDisplay.flatMap { saved in NSScreen.screens.first { displayID($0) == saved.uint32Value } }
        guard let screen = preferred ?? NSScreen.screens.first(where: { cameraHousing($0).top > 0 }) ?? NSScreen.main ?? NSScreen.screens.first else { return }
        indicatorScreen = screen
        indicatorEdge = NotchDocking.edge(in: defaults)
        indicatorFraction = NotchDocking.fraction(in: defaults)
        let current = notchGeometry(for: screen)
        geometry = current
        notchHolder = holder(of: current)
        isDocked = NotchDocking.isDocked(in: defaults, hasNotch: current.hasNotch && !notchTaken)
        model.isNotchDocked = isDocked
        presentation.backdrop.docked = isDocked
        indicatorControl?.isDocked = isDocked
        if isDocked { indicatorEdge = .top }
        model.indicatorEdge = indicatorEdge.rawValue
        screenFrame = screen.frame
        // The strip whose windows are always-present chrome (menu bar, status items), never something to yield to.
        menuBarHeight = max(steadyMenuBarHeight(on: screen), cameraHousing(screen).top, NSStatusBar.system.thickness)
        model.handleWidth = current.handleWidth
        model.hasHardwareNotch = current.hasNotch
        // The camera housing, or a virtual notch over the menu bar's centre: both begin at the screen's top edge.
        model.cutoutHeight = max(40, current.cutoutFrame.height)
        cutoutFrame = current.cutoutFrame
        indicatorFrame = isDocked ? NotchDocking.dragRegion(cutout: cutoutFrame) : indicatorFrame(on: screen)
        if isDocked { pillResting = false }
        // A resting line keeps resting where the layout puts it; the pill otherwise.
        indicatorPanel.setFrame(pillResting ? restingFrame(on: screen) : indicatorFrame, display: true)
        if indicatorControl?.isAttached != true { orderInOverlay(indicatorPanel) }
        placeEdgeRow()
        tracerPanel.setFrame(frame(for: CGSize(width: model.handleWidth + 40,
                                               height: model.cutoutHeight + 24)), display: true)
        pointerIsInside = false
        present(animate: false, requestFocus: false, caller: "updateScreen")
        queueTimerActivity()
    }

    /// The window over the handle's topmost point row, which the window server would give the menu bar: docked, the
    /// camera housing's (APP-710), closed or open; undocked on the top edge, the pill's or its resting line's
    /// (APP-730), on its own or drawn in the open surface. Out during a drag or docking (the handle moves), on any
    /// other edge, and before a layout.
    private func placeEdgeRow() {
        let row: CGRect?
        if isDocked {
            row = NotchDocking.edgeRow(cutout: cutoutFrame)
        } else if indicatorEdge == .top, indicatorDragPointer == nil, dockingRevision == nil,
                  let screen = indicatorScreen, geometry != nil {
            row = pillResting && indicatorControl?.isAttached != true
                ? NotchDocking.edgeRow(over: restingFrame(on: screen), cornerRadius: 0)
                : NotchDocking.edgeRow(over: indicatorFrame, cornerRadius: indicatorFrame.height / 2)
        } else {
            row = nil
        }
        guard let row, row.width > 0 else { edgeRowPanel.orderOut(nil); return }
        edgeRowPanel.setFrame(row, display: true)
        orderInOverlay(edgeRowPanel)
    }

    /// What another app's windows cover over this geometry's hardware notch now (AppKit coordinates), or nil (one
    /// window-list read, no titles).
    private func holder(of geometry: NotchGeometry) -> CGRect? {
        guard geometry.hasNotch else { return nil }
        let height = primaryDisplayHeight
        let cutout = NotchOverlap.windowServerRect(geometry.cutoutFrame, primaryDisplayHeight: height)
        return NotchOverlap.notchHolder(in: ScreenWindow.onScreen(), cutout: cutout, ownPIDs: [getpid()],
                                        ferbLayer: indicatorPanel.level.rawValue)
            .map { NotchOverlap.windowServerRect($0, primaryDisplayHeight: height) }   // the same flip back
    }

    /// A notch app can start or quit at any time (often at login, after FERB): look again once its windows are up.
    @objc private func applicationsChanged() {
        notchTakenRecheck.forEach { $0.cancel() }
        notchTakenRecheck = [1.5, 5].map { delay in
            let work = DispatchWorkItem { [weak self] in
                guard let self, let geometry = self.geometry,
                      (self.holder(of: geometry) == nil) != (self.notchHolder == nil) else { return }
                self.updateScreen()
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: work)
            return work
        }
    }

    /// The displays woke: lay out again only if the screens changed while they slept, order in the panel only while
    /// FERB's surface shows (never an empty closed panel, APP-729), and keep every window in the overlay space.
    @objc private func screenDidWake() {
        screensAsleep = false
        let changed = currentLayout() != appliedLayout
        Self.spacesLog.notice("""
            Displays woke: layout \(changed ? "changed" : "unchanged", privacy: .public); \(self.stateLine, privacy: .public)
            """)
        if changed { updateScreen() }
        if notchSurfaceVisible { windowYield.showPanel() }
        if indicatorControl?.isAttached != true { orderInOverlay(indicatorPanel) }
        placeEdgeRow()
        overlaySpace.ensureAll(except: model.isYielded ? [panel] : [])
        panel.resetCursorRects()
        updateTracerVisibility()
        updateYieldWatch()
        queueTimerActivity()                    // the countdown comes back by itself (APP-766)
    }

    @objc func screensDidSleep() {
        cancelMagneticInteraction()
        model.acknowledgement.dismissTransient()
        screensAsleep = true
        updateYieldWatch()
        queueTimerActivity()
    }

    @objc private func applicationActivated(_ notification: Notification) {
        guard let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication else { return }
        // macOS activates the new Space's front app just before it posts the Space change: the Space showing then
        // already differs from the one FERB last saw. That app wasn't brought forward over FERB (APP-729).
        let space = activeSpace()
        let bySpaceChange = space != nil && lastSpace != nil && space != lastSpace
        if bySpaceChange {
            Self.spacesLog.notice("""
                App \(app.processIdentifier, privacy: .public) became active with a Space change \
                (\(self.lastSpace ?? 0, privacy: .public) → \(space ?? 0, privacy: .public)): no yield; \
                \(self.stateLine, privacy: .public)
                """)
        }
        chatWindow?.otherAppActivated(app.processIdentifier, bySpaceChange: bySpaceChange)
        let cameBack = yieldController.appActivated(app.processIdentifier, bySpaceChange: bySpaceChange)
        // Another app came forward: coming back must not take the focus the owner moved there.
        if app.processIdentifier != getpid() { windowYield.ownerMovedElsewhere() }
        if cameBack { updatePointer() }
        restackYield()
    }

    /// Space changes are not display changes (APP-562, APP-729): FERB stays exactly as it is. No layout, no
    /// presentation, no reordering, no focus change (never resignKey: that discarded input focus on every swipe), and
    /// the planning preview, a reminder arrival, a pending hover and a scroll request all survive. The windows are only
    /// kept in the overlay space, on no ordinary Space, so they never ride the swipe; with the AppKit fallback the
    /// all-Spaces windows follow by themselves and the panel is ordered in only while FERB's surface shows. Hover then
    /// waits for the pointer to really move, and windows already over FERB on the new Space don't make it step aside.
    @objc func activeSpaceChanged() {
        let space = activeSpace()
        Self.spacesLog.notice("""
            Space changed \(self.lastSpace ?? 0, privacy: .public) → \(space ?? 0, privacy: .public): \
            \(self.stateLine, privacy: .public); windows \(self.overlaySpace.describe(), privacy: .public)
            """)
        lastSpace = space
        pointerAtSpaceChange = pointerLocation()
        yieldController.spaceChanged()
        chatWindow?.spaceChanged()
        if overlaySpace.isAttached {
            overlaySpace.ensureAll(except: model.isYielded ? [panel] : [])
        } else if notchSurfaceVisible {
            windowYield.showPanel()
            updateTracerVisibility()
        }
        // On a full-screen app's Space the closed edge pill is its resting line (D14).
        if !notchSurfaceVisible, !pointerIsOverSurface, onFullScreenSpace, !pillResting {
            indicatorRestTask?.cancel()
            indicatorRestTask = nil
            setIndicatorResting(true)
        }
        queueTimerActivity()
    }

    /// A full-screen app's Space shows on FERB's display.
    private var onFullScreenSpace: Bool {
        guard let screen = indicatorScreen, let id = displayID(screen) else { return false }
        return fullScreenSpace(id)
    }

    // MARK: - Where FERB opens (D19–D23, APP-745)

    /// The notch shows FERB's surface: the chat while the notch holds it, a tour line, a plan preview or a reminder
    /// arrival. While the sticky window holds the chat the notch shows none of it.
    private var notchSurfaceVisible: Bool { !model.chatInWindow && model.surfaceVisible }
    private var notchBodyVisible: Bool { !model.chatInWindow && model.bodyVisible }

    /// The chat opens in the sticky window: Where FERB opens is Window and no tour runs (the tour always runs in the
    /// notch; window mode returns after it).
    private var wantsChatInWindow: Bool { ChatHost.setting(defaults) == .window && !model.tourRunning }

    /// Applies Where FERB opens, Keep on top and Hide the pill as they are now. Switching while the chat is open moves
    /// it: the same NotchModel, draft, attachments and transcript place, now shown by the other host, without taking
    /// keyboard focus. Runs at launch, on every defaults change and when the tour starts or ends.
    func applyChatHost() {
        let inWindow = wantsChatInWindow
        let hidden = inWindow && ChatHost.hidesPill(defaults)
        let moved = model.chatInWindow != inWindow
        chatWindow?.settingsChanged()
        if moved {
            Self.spacesLog.notice("The chat moves to the \(inWindow ? "window" : "notch", privacy: .public): \(self.stateLine, privacy: .public)")
            hoverTask?.cancel()
            model.chatInWindow = inWindow
            if !inWindow { chatWindow?.hide() }
            if !model.isYielded, windowYield.isYielded { windowYield.comeBack(refocus: false) }
        }
        if hidden != pillHidden {
            pillHidden = hidden
            Self.spacesLog.notice("The pill is \(hidden ? "hidden" : "shown", privacy: .public) (Hide the pill)")
            if hidden {
                indicatorPanel.orderOut(nil)
                edgeRowPanel.orderOut(nil)
                indicatorRestTask?.cancel()
                indicatorRestTask = nil
            } else if geometry != nil {
                if indicatorControl?.isAttached != true { orderInOverlay(indicatorPanel) }
                placeEdgeRow()
            }
            updateStatusItem()
            queueTimerActivity()
        }
        if moved { present(animate: false, requestFocus: false, caller: "chat host") }
    }

    /// The sticky window follows the chat: shown while it is open, hidden when it closes. Opening shows it on the
    /// pointer's display; the owner's own open makes it key (APP-553), FERB's arrivals bring it in front without
    /// keyboard focus (D22), and other updates leave its place in the window stack alone.
    private func presentWindow(requestFocus: Bool) {
        defer { arrivalPending = false }
        guard model.chatInWindow, model.expanded else {
            chatWindow?.hide()
            return
        }
        let window = ensureChatWindow()
        let opening = !window.isShown
        let focus = requestFocus && !model.voiceIsHeld && (opening || window.panel.isKeyWindow)
        if opening || arrivalPending || focus {
            Self.spacesLog.notice("""
                Window \(opening ? "opens" : "comes forward", privacy: .public) (focus \(focus, privacy: .public), \
                arrival \(self.arrivalPending, privacy: .public))
                """)
            window.show(focus: focus)
        }
        if opening {
            model.noteTextPresentation(expanded: true)
            model.completeTextReveal()
        }
        if focus {
            DispatchQueue.main.async { [weak self] in
                guard let self, self.model.chatInWindow, self.model.bodyVisible else { return }
                self.model.focusRequest = UUID()
            }
        }
    }

    private func ensureChatWindow() -> ChatWindowController {
        if let chatWindow { return chatWindow }
        let window = ChatWindowController(model: model, defaults: defaults)
        // Fixed on every Space like the notch (APP-760): in the overlay space when FERB has one
        if overlaySpace.isAttached { window.space = overlaySpace }
        window.onClose = { [weak self] in
            guard let self, self.model.expanded else { return }
            self.model.collapse(keyboard: true)
        }
        chatWindow = window
        return window
    }

    /// The owner opened FERB in window mode (shortcut, menu, reopening FERB.app): the window opens, or comes forward,
    /// with focused typing; a chat a shortcut opened stays like a press-opened one.
    private func openWindowByOwner() {
        if model.expanded {
            beginInteraction()
            presentWindow(requestFocus: true)
            if chatWindow?.panel.isKeyWindow != true { chatWindow?.show(focus: true) }
            model.focusRequest = UUID()
        } else {
            model.toggle(byPress: true)
        }
    }

    private func present(animate: Bool, requestFocus: Bool = true, caller: String) {
        if model.chatInWindow || chatWindow?.isShown == true { presentWindow(requestFocus: requestFocus) }
        // Explicit close must win even while a pointer/fusion owns geometry.
        if requestFocus && !model.chatInWindow && !model.surfaceVisible && hasMagneticInteraction {
            cancelMagneticInteraction()
        }
        guard geometry != nil, indicatorDragPointer == nil, dockingRevision == nil else { return }
        presentCount += 1
        // Background updates (task progress) present often; they're kept in the log only when they open or close the
        // surface, so a Space change's absence of presents stays easy to read.
        let line = """
            Present (\(caller), animate \(animate), focus \(requestFocus)) surface \(notchSurfaceVisible): \(stateLine)
            """
        if caller != "background" || notchSurfaceVisible != presentedExpanded {
            Self.spacesLog.notice("\(line, privacy: .public)")
        } else {
            Self.spacesLog.debug("\(line, privacy: .public)")
        }
        presentation.compactSize = isDocked ? cutoutFrame.size : CGSize(width: 58, height: 30)
        model.noteTextPresentation(expanded: model.surfaceVisible)
        let wasExpanded = presentedExpanded
        presentedExpanded = notchSurfaceVisible
        let isOpening = notchBodyVisible && !presentedBodyVisible
        presentedBodyVisible = notchBodyVisible
        // The owner opened or revealed FERB (background updates don't request focus): stay over the windows
        // already there rather than stepping aside right after taking focus.
        if isOpening && requestFocus { yieldController.ownerOpened() }
        guard notchSurfaceVisible else {
            presentedBodyVisible = false
            updateTracerVisibility()
            updateYieldWatch()
            // On a full-screen app's Space the closed edge pill stays its resting line (D14): nothing un-rests it but
            // the owner reaching for it.
            let restsHere = onFullScreenSpace
            if !restsHere { setIndicatorResting(false) }
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
                                     anchor: anchor, animate: true) { [weak self] in
                    guard let self, self.presentationRevision == revision else { return }
                    self.detachIndicator()
                    self.panel.setFrame(self.isDocked ? NotchDocking.dragRegion(cutout: self.cutoutFrame) : self.indicatorFrame(on: screen), display: true)
                    self.panel.orderOut(nil)
                    self.model.finishCompactPreviewDismissal()
                    self.orderInOverlay(self.indicatorPanel)
                    if restsHere && !self.pointerIsInside { self.setIndicatorResting(true) }
                    else { self.scheduleIndicatorRest() }
                    self.placeEdgeRow()
                }
                return
            }
            detachIndicator()
            panel.orderOut(nil)
            model.finishCompactPreviewDismissal()
            if restsHere && !pillResting && !pointerIsInside { setIndicatorResting(true, animated: false) }
            else { scheduleIndicatorRest() }
            if indicatorControl?.isAttached != true { orderInOverlay(indicatorPanel) }
            placeEdgeRow()
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

        presentation.isIntro = model.introLine != nil
        presentation.cutoutHeight = model.cutoutHeight
        panel.setFrame(targetFrame, display: true)
        attachIndicator()
        presentation.destination = CGPoint(x: indicatorEdge == .left ? 56 : 8,
                                            y: isDocked ? 0 : (indicatorEdge == .top ? 28 : (indicatorEdge == .bottom ? 12 : 0)))
        let anchor = CGPoint(x: indicatorFrame.midX - panel.frame.minX,
                             y: isDocked ? cutoutFrame.height / 2 : panel.frame.maxY - indicatorFrame.midY)
        presentation.present(size: targetSize, expanded: notchSurfaceVisible,
                             anchor: wasExpanded ? nil : anchor,
                             animate: shouldAnimate && !reduceMotion) { [weak self] in
            guard let self, self.presentationRevision == revision else { return }
            if self.indicatorDragPointer == nil { self.panel.setFrame(targetFrame, display: true) }
            self.model.completeTextReveal()
            self.hostingView?.scheduleTextDisplayRefresh()
        }
        windowYield.showPanel()
        placeEdgeRow()
        panel.resetCursorRects()
        updateTracerVisibility()
        updateYieldWatch()
        if !notchBodyVisible {
            panel.makeFirstResponder(nil)
            panel.resignKey()
        } else if requestFocus && !model.voiceIsHeld && !model.isYielded && (isOpening || panel.isKeyWindow) {
            // The held voice chord leaves the current application's keyboard
            // focus intact until release. Background task updates also leave
            // focus with the user's active app, including after a Space change.
            // An invisible (yielded) panel never takes focus.
            panel.makeKey()
            overlaySpace.ensure(panel)
            panel.contentView?.layoutSubtreeIfNeeded()
            DispatchQueue.main.async { [weak self] in
                guard let self, self.notchBodyVisible, self.presentationRevision == revision else { return }
                self.model.focusRequest = UUID()
            }
        }
    }

    private func frame(for size: CGSize) -> CGRect {
        if notchSurfaceVisible, let screen = indicatorScreen {
            return PanelLayout.frame(size: size, screen: screen.frame, indicator: indicatorFrame,
                                     edge: indicatorEdge, docked: isDocked, cutout: cutoutFrame)
        }
        return CGRect(x: cutoutFrame.midX - size.width / 2, y: cutoutFrame.maxY - size.height,
                      width: size.width, height: size.height)
    }

    /// After three idle seconds the pill settles into a home-indicator line. Both
    /// states share the same center, so hovering unfolds it in place.
    private func setIndicatorResting(_ resting: Bool, animated: Bool = true, cause: String = #function) {
        let animated = animated && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        guard !isDocked, indicatorControl?.isAttached != true, let screen = indicatorScreen else { return }
        if resting != pillResting {
            Self.spacesLog.notice("Edge pill \(resting ? "rests" : "un-rests", privacy: .public) (\(cause, privacy: .public))")
        }
        pillResting = resting
        let target = resting ? restingFrame(on: screen) : indicatorFrame(on: screen)
        indicatorControl?.setResting(resting, animated: animated)
        if indicatorPanel.frame != target {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = animated ? 0.38 : 0
                context.timingFunction = CAMediaTimingFunction(controlPoints: 0.2, 0.8, 0.2, 1)
                indicatorPanel.animator().setFrame(target, display: true)
            }
        }
        if indicatorControl?.isAttached != true { orderInOverlay(indicatorPanel) }
        placeEdgeRow()
        queueTimerActivity()
    }

    /// The pill's home-indicator line on its edge, centred where the pill is.
    private func restingFrame(on screen: NSScreen) -> CGRect {
        let pill = indicatorFrame(on: screen)
        let length = cutoutFrame.width / 2
        switch indicatorEdge {
        case .top:
            return CGRect(x: safeTopIndicatorX(pill.midX - length / 2, size: CGSize(width: length, height: 5), screen: screen), y: screen.frame.maxY - 5, width: length, height: 5)
        case .bottom:
            return CGRect(x: min(screen.frame.maxX - length, max(screen.frame.minX, pill.midX - length / 2)), y: screen.frame.minY, width: length, height: 5)
        case .left:
            return CGRect(x: screen.frame.minX, y: min(screen.frame.maxY - length, max(screen.frame.minY, pill.midY - length / 2)), width: 5, height: length)
        case .right:
            return CGRect(x: screen.frame.maxX - 5, y: min(screen.frame.maxY - length, max(screen.frame.minY, pill.midY - length / 2)), width: 5, height: length)
        }
    }

    /// On a full-screen app's Space a running task doesn't hold the pill up (D14); a running timer holds it up only
    /// where its countdown can draw (`timerHoldsPill`, APP-766).
    private func scheduleIndicatorRest() {
        guard !isDocked, indicatorRestTask == nil, !notchSurfaceVisible,
              onFullScreenSpace || (!model.taskItems.contains(where: \.isRunning) && !timerHoldsPill)
        else { return }
        indicatorRestTask = Task { [weak self, restWait] in
            do { try await restWait(.seconds(3)) } catch { return }
            guard let self, !Task.isCancelled else { return }
            self.indicatorRestTask = nil
            guard !self.model.expanded, !self.pointerIsInside,
                  self.onFullScreenSpace || (!self.model.taskItems.contains(where: \.isRunning)
                                             && !self.timerHoldsPill) else { return }
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
    /// transparency is never touched); content, draft and attachments are untouched. Coming back re-keys the
    /// panel only if its body shows, it was key when the yield began, and the owner hasn't clicked or switched to
    /// another app meanwhile.
    private func applyYield(_ yielded: Bool) {
        // The sticky window is an ordinary floating panel: only FERB's own file picker needs it to step down (to the
        // normal level) so the picker shows above it; the notch's yield machinery never acts on it (APP-745).
        if model.chatInWindow {
            chatWindow?.filePicker(open: model.yielding.reasons.contains(.filePicker))
            if !yielded, windowYield.isYielded { windowYield.comeBack(refocus: false) }
            return
        }
        Self.spacesLog.notice("""
            Yield \(yielded ? "starts" : "ends", privacy: .public) (\(self.model.yielding.reasons.map { "\($0)" }, privacy: .public)): \
            \(self.stateLine, privacy: .public)
            """)
        if yielded {
            model.acknowledgement.dismissTransient()
            hoverTask?.cancel()
            yieldController.yieldBegan(pointer: windowServerPoint(NSEvent.mouseLocation))
            windowYield.yield(behind: yieldController.coveringWindow())
            // Left the overlay space for the ordinary Spaces (NotchOverlaySpace.leave); say so if AppKit didn't put
            // it on the current one, which would leave it off screen while it still counts as visible.
            if overlaySpace.isAttached && !panel.isOnActiveSpace {
                NSLog("FERB Notch: the stepped-back panel isn't on the current Space.")
            }
        } else {
            windowYield.comeBack(refocus: notchBodyVisible && !model.voiceIsHeld)
            overlaySpace.ensure(panel)
        }
        updateTracerVisibility()
    }

    /// FERB's drawn frames in window-server coordinates, from the model (see NotchYieldController.Layout).
    var introScreenFrame: CGRect? {
        model.introLine == nil ? nil : frame(for: model.surfaceSize)
    }

    func introCompanionFocused(_ window: Int?) { yieldController.companionFocused(window) }

    func yieldLayout() -> NotchYieldController.Layout? {
        guard geometry != nil, !screenFrame.isEmpty else { return nil }
        let height = primaryDisplayHeight
        func server(_ rect: CGRect) -> CGRect { NotchOverlap.windowServerRect(rect, primaryDisplayHeight: height) }
        // FERB's drawn surface, not the panel window: its transparent margins pass clicks through.
        var occupied: [CGRect] = []
        // Closed, nothing of the panel shows: the handle (in the camera housing, or the edge pill on its own window) is
        // never ordered out for another app, so a closed FERB never yields (APP-729).
        if notchSurfaceVisible { occupied.append(frame(for: model.surfaceSize)) }
        if model.showsTraceGlow { occupied.append(tracerPanel.frame) }
        let menuBar = CGRect(x: screenFrame.minX, y: screenFrame.maxY - menuBarHeight,
                             width: screenFrame.width, height: menuBarHeight)
        return .init(occupied: occupied.map(server),
                     tourLine: model.introLine != nil ? server(frame(for: model.surfaceSize)) : nil,
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
        // During a file drag the drag image passes over FERB: it is not a window FERB steps behind (APP-742).
        guard !fileDragActive else { return }
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
        focusVisibleInput()
        return true
    }

    /// Hover, a click on the notch and the shortcut focus the visible text input; never while FERB still yields.
    private func focusVisibleInput() {
        if windowYield.focusInput(bodyVisible: notchBodyVisible) {
            overlaySpace.ensure(panel)
            model.focusRequest = UUID()
        }
    }

    private func beginInteraction() {
        if model.compactPlanText != nil {
            if model.compactReminderCard != nil { model.openReminderChat() }
            else { model.expand() }
        }
        // The owner reached for FERB (shortcut, menu or a control): stop yielding to an app under the tour's line
        // and show FERB over the windows now over it.
        yieldController.summon()
        hoverTask?.cancel()
        indicatorRestTask?.cancel()
        indicatorRestTask = nil
        setIndicatorResting(false)
        if model.chatInWindow {
            if model.bodyVisible, let window = chatWindow, window.isShown, !window.panel.isKeyWindow {
                window.show(focus: true)
            }
        } else if model.bodyVisible, !model.isYielded {
            panel.makeKey()
            overlaySpace.ensure(panel)
        }
    }

    private var previewWindows: [UUID: NSWindow] = [:]

    private var pointerIsOverSurface: Bool {
        let point = pointerLocation()
        if model.expanded, previewWindows.values.contains(where: { $0.isVisible && $0.frame.contains(point) }) {
            return true
        }
        if isDocked, NotchDocking.accepts(point, cutout: cutoutFrame, hasHardwareNotch: model.hasHardwareNotch) {
            return true
        }
        let indicatorHitFrame = isDocked ? cutoutFrame.union(indicatorFrame) : indicatorFrame.insetBy(dx: 0, dy: -2)
        guard notchSurfaceVisible else { return indicatorHitFrame.contains(point) || indicatorPanel.frame.contains(point) }
        let panelFrame = panel.frame.insetBy(dx: 0, dy: -2)
        if panelFrame.contains(point) { return true }
        // Keep the hover trigger hot as the compact indicator is handed off to the
        // expanding surface. This also prevents an edge coordinate conversion from
        // folding FERB while the pointer is still at its origin.
        return indicatorHitFrame.contains(point)
    }

    func updatePointer() {
        guard indicatorDragPointer == nil, dockingRevision == nil else { return }
        // After a Space change nothing happens until the pointer really moves: tracking areas and windows coming back
        // on the new Space report a pointer that never moved (APP-729).
        if let held = pointerAtSpaceChange {
            guard pointerLocation() != held else { return }
            pointerAtSpaceChange = nil
        }
        // While FERB yields it is ordered out: hover must not open, reveal, fold or dismiss it. Entering the notch
        // brings it back, and like any hover that focuses the visible input.
        var broughtBack = false
        if model.isYielded {
            switch yieldController.pointerMoved(to: windowServerPoint(pointerLocation())) {
            case .hidden: return
            case .broughtBack: broughtBack = true
            case .visible: break
            }
        }
        defer { if broughtBack { focusVisibleInput() } }
        let inside = pointerIsOverSurface
        guard inside != pointerIsInside else { return }
        pointerIsInside = inside
        inside ? pointerEntered() : pointerExited()
    }

    private func pointerEntered() {
        // On its way to Mission Control's Spaces bar the pointer crosses the top edge: that isn't reaching for FERB,
        // so nothing opens, reveals or un-rests (it counts as inside, so only leaving and coming back opens).
        if missionControlShowing(NotchOverlap.windowServerRect(screenFrame, primaryDisplayHeight: primaryDisplayHeight)) {
            Self.spacesLog.notice("Pointer entered FERB while Mission Control shows: nothing opens or un-rests")
            return
        }
        hoverTask?.cancel()
        indicatorRestTask?.cancel()
        indicatorRestTask = nil
        setIndicatorResting(false)
        if model.compactPlanText != nil && model.compactReminderCard != nil {
            model.openReminderChat()
            return
        }
        // In window mode hover never opens the window; a press on the notch or pill does (APP-746).
        guard model.hoverOpensChat, !model.chatInWindow,
              !model.expanded else { return }
        hoverTask = Task { [weak self, hoverWait] in
            do { try await hoverWait(.milliseconds(100)) } catch { return }
            guard let self, !Task.isCancelled else { return }
            Self.spacesLog.notice("Hover opens the chat: \(self.stateLine, privacy: .public)")
            self.model.expand()
        }
    }

    private func pointerExited() {
        guard indicatorDragPointer == nil, dockingRevision == nil else { return }
        hoverTask?.cancel()
        // A chat the owner opened with a press stays until Escape, the shortcut, Hide or a click outside (APP-707).
        guard !model.blocksAutomaticDismissal, !model.staysOpenOnHoverOut, model.expanded else {
            scheduleIndicatorRest()
            return
        }
        // Allow the pointer to cross the popover arrow/gap without folding either
        // surface. Once outside both windows, a preview must close with the chat.
        let delay = previewWindows.isEmpty ? 60 : 250
        hoverTask = Task { [weak self, hoverWait] in
            do { try await hoverWait(.milliseconds(delay)) } catch { return }
            guard let self, !Task.isCancelled, !self.pointerIsOverSurface, !self.model.blocksAutomaticDismissal,
                  !self.model.staysOpenOnHoverOut, self.model.expanded else { return }
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
        if model.chatInWindow, model.expanded { chatWindow?.show(focus: true) }
    }

    private func queueStatusRefresh() {
        guard !statusRefreshQueued else { return }
        statusRefreshQueued = true
        DispatchQueue.main.async { [weak self] in        // objectWillChange fires before the change lands
            guard let self else { return }
            self.statusRefreshQueued = false
            self.model.discardFinishedPreviews()
            self.updateStatusItem()
            self.updateTimerActivity()
        }
    }

    // MARK: - The timer's live activity (APP-762)

    /// How wide each side of the live activity is beside the camera housing or the pill.
    static let timerEarWidth: CGFloat = 58

    /// Where the closed notch's live activity draws, the gap it leaves for the housing or the pill, and the housing or
    /// pill itself; nil when it doesn't show: no timer running, FERB open (the notch or the sticky window), an arrival or
    /// a tour line on the notch, FERB stepped back, the displays asleep, the pill hidden or resting as its thin line
    /// (APP-766), a drag or docking under way, or the pill on a side edge. Docked, it is one black notch shape across
    /// the ears and the housing (APP-771): exactly the housing's height, flush with its bottom edge, widened on each
    /// side by an ear and the shape's top shoulder.
    func timerActivityPlace() -> (frame: CGRect, gap: CGFloat, housing: CGRect)? {
        guard model.soonestTimer() != nil, !model.expanded, model.compactContent == nil, !model.isYielded,
              !screensAsleep, !pillHidden, indicatorDragPointer == nil, dockingRevision == nil, geometry != nil
        else { return nil }
        let ear = Self.timerEarWidth
        if isDocked {
            let housing = cutoutFrame
            let side = ear + TimerActivityView.shoulder(height: housing.height)
            return (housing.insetBy(dx: -side, dy: 0), housing.width, housing)
        }
        guard indicatorEdge == .top || indicatorEdge == .bottom, indicatorControl?.isAttached != true, !pillResting
        else { return nil }
        let pill = indicatorFrame                         // the pill's own place, not its window mid-animation
        return (CGRect(x: pill.minX - ear, y: pill.minY, width: pill.width + 2 * ear, height: pill.height), pill.width,
                pill)
    }

    /// A running timer holds the undocked pill up instead of its resting line only where its countdown can draw beside
    /// it: on the top or bottom edge, the pill shown, off a full-screen Space (D14) (APP-766).
    private var timerHoldsPill: Bool {
        model.soonestTimer() != nil && !isDocked && (indicatorEdge == .top || indicatorEdge == .bottom)
            && !pillHidden && !onFullScreenSpace
    }

    /// Something the live activity depends on changed (a drop, docking, the displays, a Space, the pill's rest): look
    /// again once the change has landed, once per turn of the run loop.
    private func queueTimerActivity() {
        guard !timerActivityQueued else { return }
        timerActivityQueued = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.timerActivityQueued = false
            self.updateTimerActivity()
        }
    }

    /// Shows, places or hides the live activity. A running timer holds the pill up where its countdown can draw
    /// (`timerHoldsPill`); when it ends or is cancelled, the pill is free to rest again by itself.
    func updateTimerActivity() {
        let holds = timerHoldsPill
        if holds, pillResting, !model.expanded, indicatorDragPointer == nil, dockingRevision == nil {
            indicatorRestTask?.cancel()
            indicatorRestTask = nil
            setIndicatorResting(false)
        }
        if timerHeldPill, !holds { scheduleIndicatorRest() }
        timerHeldPill = holds
        guard let place = timerActivityPlace() else {
            if timerActivityPanel.isVisible { timerActivityPanel.orderOut(nil) }
            timerActivityTick?.cancel()
            timerActivityTick = nil
            return
        }
        if place.gap != timerActivityGap || timerActivityPanel.contentView == nil {
            timerActivityGap = place.gap
            timerActivityPanel.contentView = NSHostingView(rootView: TimerActivityView(
                model: model, gap: place.gap, height: place.frame.height, pill: !isDocked))
        }
        if timerActivityPanel.frame != place.frame { timerActivityPanel.setFrame(place.frame, display: true) }
        if !timerActivityPanel.isVisible {
            // Under the handle, so what it draws at the camera still shows over the black that now runs behind the
            // camera too (APP-771); the glow's window sits a level higher. Ordered in first, then kept in the overlay
            // space, as orderInOverlay does.
            if indicatorPanel.isVisible {
                timerActivityPanel.order(.below, relativeTo: indicatorPanel.windowNumber)
            } else {
                timerActivityPanel.orderFrontRegardless()
            }
            if overlaySpace.isAttached {
                overlaySpace.include(timerActivityPanel)
                overlaySpace.ensure(timerActivityPanel)
            }
        }
        // It ends by itself when the last timer runs out: look again once a second while it shows
        if timerActivityTick == nil {
            timerActivityTick = Task { [weak self] in
                while !Task.isCancelled {
                    do { try await Task.sleep(for: .seconds(1)) } catch { return }
                    self?.updateTimerActivity()
                }
            }
        }
    }

    private func configureTimerActivity() {
        timerActivityPanel.title = "FERB timer"
        timerActivityPanel.backgroundColor = .clear
        timerActivityPanel.isOpaque = false
        timerActivityPanel.hasShadow = false
        timerActivityPanel.hidesOnDeactivate = false
        timerActivityPanel.isReleasedWhenClosed = false
        timerActivityPanel.level = indicatorPanel.level
        timerActivityPanel.collectionBehavior = Self.overlayBehavior
        timerActivityPanel.animationBehavior = .none
    }

    private func updateStatusItem() {
        let state = model.menuBarState
        statusItem?.button?.image = NSImage(systemSymbolName: state.symbol, accessibilityDescription: state.statusLine)
        statusItem?.button?.toolTip = state.statusLine
        statusItem?.button?.contentTintColor = statusTint.map { tint in
            switch tint {
            case .amber: NSColor.systemYellow
            case .red: NSColor.systemRed
            case .green: NSColor.systemGreen
            }
        }
    }

    /// With the pill hidden the menu bar icon carries the pill's status colour (D21), from the same phase.
    var statusTint: StatusTint? { StatusTint.of(model.indicatorPhase, pillHidden: pillHidden) }

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
    /// packaged app sets it up again as the service step in the notch (APP-686) and a development build says how.
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

    /// The menu bar's Open FERB and opening FERB.app again (applicationShouldHandleReopen): the notch, or the window
    /// in window mode, opened by the owner with focused typing.
    @objc func showFromMenu() {
        if model.chatInWindow { openWindowByOwner(); return }
        beginInteraction()
        model.expand()
    }

    /// The drag pasteboard (tests stand in their own) and the change count FERB last read from it.
    var dragPasteboard: () -> NSPasteboard = { NSPasteboard(name: .drag) }
    private var dragPasteboardCount = 0
    private var dragCheckedAt: TimeInterval = 0
    /// Whether the left mouse button is still down (tests end a drag without one).
    var leftButtonDown: () -> Bool = { NSEvent.pressedMouseButtons & 1 != 0 }
    /// How the drag watch's polls and the wait after the drop pass (tests end them at once).
    var fileDragWait: @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) }
    /// A file drag is under way and FERB's drop windows are out of the overlay space (APP-742).
    private(set) var fileDragActive = false
    private(set) var fileDragWatch: Task<Void, Never>?

    /// A drag moved somewhere (any app, or FERB's own window): when a new drag session put file URLs on the drag
    /// pasteboard, FERB's handle, edge row and chat leave the overlay space so the window server routes the drag to
    /// them (it never routes one into that space, APP-742), until the button is up. Read at most every 100 ms.
    func noticeFileDrag() {
        guard !fileDragActive else { return }
        let now = CACurrentMediaTime()
        guard now - dragCheckedAt >= 0.1 else { return }
        dragCheckedAt = now
        noticeDragPasteboard()
    }

    /// The drag pasteboard changed to file URLs: a file drag began.
    func noticeDragPasteboard() {
        let board = dragPasteboard()
        guard board.changeCount != dragPasteboardCount else { return }
        dragPasteboardCount = board.changeCount
        guard board.types?.contains(.fileURL) == true else { return }
        beginFileDrag()
    }

    /// The windows a file drag must reach: the notch's, and the sticky window while it shows (APP-742, APP-760).
    private var dropWindows: [NSWindow] {
        [indicatorPanel, edgeRowPanel, panel] + (chatWindow.map { $0.isShown ? [$0.panel] : [] } ?? [])
    }

    func beginFileDrag() {
        guard !fileDragActive else { return }
        fileDragActive = true
        overlaySpace.holdOut(dropWindows)
        fileDragWatch = Task { [weak self, fileDragWait] in
            while let self, !Task.isCancelled, self.leftButtonDown() {
                do { try await fileDragWait(.milliseconds(200)) } catch { return }
            }
            // The drop is delivered as the button comes up; the windows go back after it.
            do { try await fileDragWait(.milliseconds(400)) } catch { return }
            self?.endFileDrag()
        }
    }

    func endFileDrag() {
        guard fileDragActive else { return }
        fileDragActive = false
        fileDragWatch?.cancel()
        fileDragWatch = nil
        // The panel while FERB yields is out on purpose; its yield brings it back.
        overlaySpace.endHoldOut(keepOut: model.isYielded ? [panel] : [])
        model.drops.dragEnded()
    }

    /// A file drag held over the handle or pill (D17): opens the chat as a press does (APP-705/707), bringing FERB
    /// back from a yield, whatever the hover setting.
    func openForFileDrag() {
        cancelMagneticInteraction()
        if summonFromYield() { return }
        guard !model.bodyVisible else { return }
        Self.spacesLog.notice("A file drag held over FERB opens the chat: \(self.stateLine, privacy: .public)")
        model.toggle(byPress: true)
    }

    /// The modifier double-tap (when on, AppSettings.doubleTap).
    func modifierDoubleTapped() { toggleByShortcut() }

    /// Every keyboard shortcut that opens FERB (double-tap right Option, the owner's own combination): opens or closes
    /// the notch; opening it plays summon; from a yield it brings FERB back. A chat it opens stays open on hover-out,
    /// like a press-opened one (APP-709). The one place a shortcut opens FERB.
    func toggleByShortcut() {
        if model.chatInWindow {
            // Window mode (APP-746): the shortcut hides the window when it has keyboard focus, and otherwise opens it or
            // brings it forward with focused typing.
            if model.expanded, chatWindow?.isShown == true, chatWindow?.panel.isKeyWindow == true {
                model.collapse(keyboard: true)
            } else {
                openWindowByOwner()
                model.feedback.play(.summon)
            }
            introControls.saw(.toggle)
            return
        }
        if summonFromYield() { model.feedback.play(.summon) }
        else {
            model.toggle(byPress: true)
            if model.expanded { model.feedback.play(.summon) }
        }
        introControls.saw(.toggle)
    }

    @objc private func showSettings() {
        if !model.chatInWindow { model.collapse() }        // the window stays until the owner closes it (D23)
        onOpenSettings?()
    }

    private func quickLook(_ url: URL) {
        previewURL = url
        beginInteraction()
        guard let preview = QLPreviewPanel.shared() else { return }
        preview.updateController()
        preview.makeKeyAndOrderFront(nil)            // the sticky window steps down while it shows (APP-749)
        if !model.chatInWindow { model.collapse() }
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
        timerActivityTick?.cancel()
        timerActivityTick = nil
        timerActivityPanel.orderOut(nil)
        cancelMagneticInteraction()
        model.acknowledgement.dismiss()
        closeTaskActivity()
        poster?.stop()
        statusRefresh = nil
        model.disconnectDaemon()
        shortcuts.stop()
        hotKeys.stop()
        fileDragWatch?.cancel()
        if let defaultsObserver { NotificationCenter.default.removeObserver(defaultsObserver) }
        chatWindow?.hide()
        presentation.stop()
        panel.orderOut(nil)
        tracerPanel.orderOut(nil)
        indicatorPanel.orderOut(nil)
        edgeRowPanel.orderOut(nil)
        chatWindow?.space = nil
        overlaySpace.stop()
        presentationRevision = UUID()
        hoverTask?.cancel()
        indicatorRestTask?.cancel()
        yieldWatch?.cancel()
        yieldWatch = nil
        yieldController.stop()
        eventMonitors.forEach(NSEvent.removeMonitor)
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        if let statusItem { NSStatusBar.system.removeStatusItem(statusItem) }
    }
}
