import AppKit
import OSLog
import SwiftUI
import UniformTypeIdentifiers
import Quartz
import NotchCore

/// The sticky window (APP-745, owner's item 8 "Sticky window, like ChatGPT old app"): FERB's chat in a black-glass
/// window that stays open until the owner closes it (close button, ⌘W or Esc). A non-activating panel, so FERB never
/// becomes the active app, stays out of the Dock and ⌘-Tab (D23) and keyboard focus moves to it only when the owner
/// opens or clicks it; on every Space, full-screen ones included (D20).
final class ChatWindowPanel: NSPanel {
    var onClose: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    /// Esc hides the window like its close button (D23), except while a field composes text with an input method
    /// (the input method takes it). An open menu runs its own event loop, so its Esc never arrives here. A click while
    /// another app is in front makes the window key first, so the click itself reaches what was clicked (a button, the
    /// profile menu, a field) instead of only bringing the window to keyboard focus (APP-759).
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown, Self.isPlainEscape(event), !composingText {
            onClose?()
            return
        }
        if [.leftMouseDown, .rightMouseDown].contains(event.type), !isKeyWindow { makeKey() }
        super.sendEvent(event)
    }

    override func cancelOperation(_ sender: Any?) { if !composingText { onClose?() } }
    override func performClose(_ sender: Any?) { onClose?() }

    private var composingText: Bool { (firstResponder as? NSTextView)?.hasMarkedText() == true }

    static func isPlainEscape(_ event: NSEvent) -> Bool {
        event.keyCode == 53
            && event.modifierFlags.intersection(.deviceIndependentFlagsMask).subtracting([.capsLock, .numericPad]).isEmpty
    }

    /// ⌘W hides the window (D23); ⌘C X A V Z and ⇧⌘Z reach the focused field as in the notch (APP-717); other
    /// Command keys go to FERB's main menu, which never sees this window's keys because FERB never activates (⌘Q
    /// quits, ⌘, opens Settings).
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask).subtracting([.capsLock, .numericPad])
        if flags == .command, event.charactersIgnoringModifiers?.lowercased() == "w" {
            onClose?()
            return true
        }
        if let action = NotchPanel.editAction(for: event), let responder = firstResponder,
           (responder as? NSView)?.isHiddenOrHasHiddenAncestor != true,
           NotchPanel.canPerform(action, undo: responder.undoManager),
           responder.tryToPerform(action, with: self) { return true }
        if super.performKeyEquivalent(with: event) { return true }
        if flags.contains(.command), NSApp.mainMenu?.performKeyEquivalent(with: event) == true { return true }
        return false
    }
}

/// FERB handing the owner something in another app: a file shown in Finder, a document, link or site opened. Every
/// such hand-off goes through here, so the sticky window steps down from Keep on top and what opened shows above it,
/// until the owner comes back to the window (APP-749).
enum HandOff {
    static let notification = Notification.Name("FERBHandOff")

    static func announce() { NotificationCenter.default.post(name: notification, object: nil) }

    @discardableResult
    static func open(_ url: URL) -> Bool {
        announce()
        return NSWorkspace.shared.open(url)
    }

    static func reveal(_ urls: [URL]) {
        announce()
        NSWorkspace.shared.activateFileViewerSelecting(urls)
    }
}

/// The window's state its header and level read: Keep on top (D20; set only in Settings, APP-758) and close.
@MainActor
final class ChatWindowControls: ObservableObject {
    @Published var keepOnTop: Bool
    var close: () -> Void = {}
    init(keepOnTop: Bool) { self.keepOnTop = keepOnTop }
}

/// Opens, hides, places and levels the sticky window. Created the first time window mode shows it, never otherwise.
///
/// Where it lives (owner, 2026-11-06, APP-760): in FERB's fixed overlay space, like the notch, so a Space switch
/// (Control-arrow, a swipe, Mission Control, a full-screen Space) never moves it. It steps back into the ordinary window
/// stack, one layer back like the notch's yield, while something must show above it: FERB's own file picker, Settings,
/// alerts or Quick Look (APP-749), what FERB just opened in another app, another process's menu, popover or floating
/// window over it, a file drag (APP-742's hold-out) and, with Keep on top off, another app's window the owner brought
/// forward over it. It comes back when none of those holds. Without the overlay space (SkyLight unavailable) it is an
/// ordinary all-Spaces panel whose level follows Keep on top, as before.
@MainActor
final class ChatWindowController: NSObject, NSWindowDelegate {
    let panel: ChatWindowPanel
    let controls: ChatWindowControls
    private let model: NotchModel
    private let defaults: UserDefaults
    /// The owner closed the window (close button, ⌘W, Esc): the coordinator closes the chat.
    var onClose: (() -> Void)?
    /// The displays (tests stand in their own frames and ids).
    var screens: () -> [(id: UInt32?, visible: CGRect)] = {
        NSScreen.screens.map { screen in
            ((screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value,
             screen.visibleFrame)
        }
    }
    /// Where the pointer is: the window opens on that display, at its place there.
    var pointer: () -> CGPoint = { NSEvent.mouseLocation }
    /// Placing the window itself (open, display change) is not the owner moving it.
    private var placing = false
    /// FERB's own file picker is open: the window steps down to the normal level so the picker shows above it.
    private var underPicker = false
    /// Another of FERB's own titled windows is on screen (Settings and the FERB window, an alert, Quick Look, a file
    /// picker): the window steps down so it shows above (APP-749).
    private var underOwnWindow = false
    /// FERB handed the owner something in another app (`HandOff`): the window stays down until the owner comes back
    /// to it (it becomes key) or FERB shows it again.
    private var handedOff = false
    /// Another process's menu, popover or floating window (one the owner hasn't shown the window above) is over it.
    private(set) var overlaysOver = false
    /// Keep on top off: the app whose window the owner brought forward over the window.
    private(set) var coveredBy: Int32?
    /// Other processes' windows the owner chose to show the window above (over it when they opened it or clicked into
    /// it), until they leave the screen; the notch's rule (NotchYieldController.overriddenOverlays).
    private var overridden = Set<UInt32>()
    private var overrideAtNextCheck = false
    /// The window stepped back into the ordinary window stack (out of the overlay space).
    private(set) var steppedBack = false
    private var observers: [NSObjectProtocol] = []
    private var watch: Task<Void, Never>?
    private var activationRecheck: Task<Void, Never>?
    private var systemInput: [Int32: Bool] = [:]

    /// FERB's fixed overlay space (the notch's); nil or detached: the AppKit fallback.
    var space: NotchOverlaySpace? {
        didSet { applyPlace() }
    }
    /// The on-screen windows (tests stand in their own rows).
    var windowList: () -> [ScreenWindow] = ScreenWindow.onScreen
    /// Whether a window belongs only to Spaces other than the one showing (mid-swipe), as the notch reads it.
    var onAnotherSpace: (UInt32) -> Bool = { number in
        let spaces = WindowServerSpaces.spaces(ofWindow: Int(number))
        guard !spaces.isEmpty, let active = WindowServerSpaces.activeSpace() else { return false }
        return !spaces.contains(active)
    }
    var ownPIDs: Set<Int32> = [getpid()]
    var bundleID: (Int32) -> String? = { NSRunningApplication(processIdentifier: $0)?.bundleIdentifier }
    var isWindowManager: (Int32) -> Bool = {
        NSRunningApplication(processIdentifier: $0)?.bundleIdentifier == "com.apple.WindowManager"
    }
    static let watchInterval: Duration = .milliseconds(250)
    /// How long after a Space change an app's activation still counts as that Space's own front app coming forward:
    /// macOS posts the activation before or after the Space change (live probe, macOS 27.2: after, on Control-arrow).
    static let spaceChangeGrace: TimeInterval = 1.0
    var now: () -> Date = { Date() }
    /// The Space showing (tests stand in their own).
    var activeSpace: () -> UInt64? = { WindowServerSpaces.activeSpace() }
    private var lastSpaceChange: Date?
    /// The Space showing at the last look: a different one now is a Space change, whether or not its notification
    /// has arrived yet.
    private var seenSpace: UInt64?

    /// Notes a Space change seen by its notification or by the Space showing having changed since the last look.
    private func noteSpace(changed: Bool = false) {
        let space = activeSpace()
        if changed || (space != nil && seenSpace != nil && space != seenSpace) { lastSpaceChange = now() }
        if space != nil { seenSpace = space }
    }

    static let behavior: NSWindow.CollectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]

    init(model: NotchModel, defaults: UserDefaults) {
        self.model = model
        self.defaults = defaults
        controls = ChatWindowControls(keepOnTop: ChatHost.keepsOnTop(defaults))
        panel = ChatWindowPanel(contentRect: CGRect(origin: .zero, size: ChatWindowFrames.standardSize),
                                styleMask: [.titled, .closable, .resizable, .fullSizeContentView, .nonactivatingPanel],
                                backing: .buffered, defer: false)
        super.init()
        panel.title = "FERB"
        panel.identifier = NSUserInterfaceItemIdentifier("FERBChatWindow")
        panel.titleVisibility = .hidden
        panel.titlebarAppearsTransparent = true
        for button in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            panel.standardWindowButton(button)?.isHidden = true
        }
        panel.isFloatingPanel = false
        // The header sits under the window's transparent title bar. A movable window's title bar is a window-server drag
        // region: a press on a header control there could start a window drag in the window server, and the control
        // never heard its click (the profile menu flickered or ignored it, owner 2026-11-06). Not movable by the window
        // server, every press reaches the app; the header's empty space moves the window itself (WindowDragArea), and
        // a display change re-places it (screensChanged) (APP-759).
        panel.isMovable = false
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = false
        panel.isReleasedWhenClosed = false
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.appearance = NSAppearance(named: .darkAqua)
        panel.collectionBehavior = Self.behavior
        panel.minSize = ChatWindowFrames.minimumSize
        panel.delegate = self
        panel.onClose = { [weak self] in self?.onClose?() }
        controls.close = { [weak self] in self?.onClose?() }
        let hosting = NSHostingView(rootView: ChatWindowView(model: model, controls: controls))
        hosting.sizingOptions = []
        panel.contentView = hosting
        applyPlace()
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: HandOff.notification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.handOff() }
        })
        observers.append(center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: panel, queue: .main) {
            [weak self] _ in MainActor.assumeIsolated { self?.ownerReturned() }
        })
        // FERB's other windows coming and going (the same notifications DockPresence counts them by)
        for name in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification,
                     NSWindow.willCloseNotification, NSWindow.didMiniaturizeNotification,
                     NSWindow.didDeminiaturizeNotification, NSWindow.didChangeOcclusionStateNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.ownWindowsChanged() }
            })
        }
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
        watch?.cancel()
        activationRecheck?.cancel()
    }

    var isShown: Bool { panel.isVisible }

    /// Shows the window: placed on the pointer's display at its saved place when it wasn't showing, brought in front
    /// of other windows without taking keyboard focus (D22), and made key only for `focus` (the owner opened it). The
    /// owner (or FERB's arrival) showing it is the owner's choice of it over what was in front: it comes forward.
    func show(focus: Bool) {
        handedOff = false
        coveredBy = nil
        overrideAtNextCheck = true
        if !panel.isVisible { place() }
        panel.orderFrontRegardless()
        applyPlace()
        // in the space, or, stepped back (perhaps while it was hidden), out of it on the Space showing (APP-767)
        if fixed { if steppedBack { space?.stepBack(panel) } else { space?.stepForward(panel) } }
        if focus { panel.makeKey() }
        startWatch()
    }

    func hide() {
        guard panel.isVisible else { return }
        panel.orderOut(nil)
        watch?.cancel()
        watch = nil
    }

    /// Keep on top (on: floating; off: among other windows) read again, e.g. after Settings changed it.
    func settingsChanged() {
        controls.keepOnTop = ChatHost.keepsOnTop(defaults)
        if controls.keepOnTop { coveredBy = nil }
        applyPlace()
    }

    /// FERB's file picker opened or closed (it must show above the window).
    func filePicker(open: Bool) {
        underPicker = open
        applyPlace()
    }

    /// FERB opened something in another app: what opened shows above the window.
    func handOff() {
        handedOff = true
        applyPlace()
    }

    /// The owner came back to the window (it became key): nothing they had brought forward holds it back any more.
    private func ownerReturned() {
        handedOff = false
        coveredBy = nil
        overrideAtNextCheck = true
        applyPlace()
    }

    // MARK: - Fixed on every Space (APP-760)

    private var fixed: Bool { space?.isAttached == true }

    /// What must show above the window now.
    var stepsBack: Bool { underPicker || underOwnWindow || handedOff || overlaysOver || coveredBy != nil }

    /// The window's frame in window-server coordinates (origin at the primary display's top left).
    private var serverFrame: CGRect {
        NotchOverlap.windowServerRect(panel.frame, primaryDisplayHeight: NSScreen.screens.first?.frame.maxY ?? 0)
    }

    /// An app came forward (`bySpaceChange`: the new Space's own front app, which nobody brought over the window).
    /// With Keep on top off, the window steps behind that app's window when one lies over it; a window that unhides
    /// or un-minimizes with its app shows a moment later, so it is looked for again then.
    func otherAppActivated(_ pid: Int32, bySpaceChange: Bool) {
        activationRecheck?.cancel()
        noteSpace()
        let afterSpaceChange = lastSpaceChange.map { now().timeIntervalSince($0) < Self.spaceChangeGrace } ?? false
        guard !bySpaceChange, !afterSpaceChange, !ownPIDs.contains(pid), !controls.keepOnTop, isShown, fixed
        else { return }
        if coverIfUnder(pid) { return }
        activationRecheck = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(500)) } catch { return }
            self?.coverIfUnder(pid)
        }
    }

    /// The owner clicked another app (FERB's global monitor; `point` in window-server coordinates): with Keep on top
    /// off, an app already in front whose window lies under the window takes the front as an activation would.
    func ownerClicked(at point: CGPoint) {
        guard !controls.keepOnTop, isShown, fixed else { return }
        let rows = windowList()
        if let app = NotchOverlap.clickedApp(at: point, in: rows, ownPIDs: ownPIDs,
                                             canBeActive: { _ in true }, frontmost: { nil }) {
            coverIfUnder(app, rows: rows)
        }
    }

    @discardableResult
    private func coverIfUnder(_ pid: Int32, rows: [ScreenWindow]? = nil) -> Bool {
        let rows = (rows ?? windowList()).filter { !onAnotherSpace($0.windowID) }
        guard let front = NotchOverlap.frontWindowUnderChat(in: rows, appPID: pid, chat: serverFrame) else { return false }
        coveredBy = pid
        applyPlace()
        if panel.isVisible { panel.order(.below, relativeTo: Int(front.windowID)) }
        return true
    }

    /// A Space change: nothing moves; what holds the window back is read again on the Space showing now.
    func spaceChanged() {
        noteSpace(changed: true)
        activationRecheck?.cancel()
        evaluate()
    }

    private func startWatch() {
        guard fixed, watch == nil else { return }
        watch = Task { [weak self] in
            repeat {
                self?.evaluate()
                do { try await Task.sleep(for: Self.watchInterval, tolerance: .milliseconds(50)) } catch { return }
            } while self != nil
        }
    }

    /// One look at the windows around the sticky window: another process's menu, popover or floating window over it
    /// (not one the owner chose to show it above; never during Mission Control or for system text input), and, with
    /// Keep on top off, whether the app it stepped behind still has a window over it.
    func evaluate() {
        guard isShown, fixed, let screen = panel.screen ?? NSScreen.main else { return }
        noteSpace()
        let height = NSScreen.screens.first?.frame.maxY ?? screen.frame.maxY
        func server(_ rect: CGRect) -> CGRect { NotchOverlap.windowServerRect(rect, primaryDisplayHeight: height) }
        let bar = max(0, screen.frame.maxY - screen.visibleFrame.maxY)
        let display = server(screen.frame)
        let menuBar = server(CGRect(x: screen.frame.minX, y: screen.frame.maxY - bar, width: screen.frame.width,
                                    height: bar))
        let rows = windowList()
        let missionControl = SpaceFacts.missionControlShowing(in: rows, display: display) { [unowned self] in
            isWindowManager($0)
        }
        let frame = serverFrame
        let over = missionControl ? [] : NotchOverlap.foreignOverlays(in: rows, ferbFrames: [frame], ownPIDs: ownPIDs,
                                                                      menuBar: menuBar, display: display)
            .filter { !isSystemInput($0.ownerPID) && !onAnotherSpace($0.windowID) }
        let ids = Set(over.map(\.windowID))
        overridden.formIntersection(rows.map(\.windowID))
        if overrideAtNextCheck { overridden.formUnion(ids) }
        overrideAtNextCheck = false
        var changed = false
        let overlays = !ids.isSubset(of: overridden)
        if overlays != overlaysOver {
            overlaysOver = overlays
            changed = true
        }
        if let app = coveredBy, !missionControl,
           NotchOverlap.frontWindowUnderChat(in: rows.filter { !onAnotherSpace($0.windowID) }, appPID: app,
                                             chat: frame) == nil {
            coveredBy = nil
            changed = true
        }
        if changed { applyPlace() }
    }

    private func isSystemInput(_ pid: Int32) -> Bool {
        if let known = systemInput[pid] { return known }
        let input = bundleID(pid).map(NotchOverlap.isSystemInput(bundleID:)) ?? false
        systemInput[pid] = input
        return input
    }

    /// Whether another of FERB's own windows the owner works in is showing: a titled one (Settings and the FERB
    /// window, an alert, a file picker) or Quick Look's panel (untitled). Counted after the current event, so a closing
    /// one has left the screen; structure only, never a window's title or content. The notch's own borderless panels
    /// never count.
    static func coveredByOwnWindow(_ windows: [(isChat: Bool, isTitled: Bool, isVisible: Bool)]) -> Bool {
        windows.contains { !$0.isChat && $0.isTitled && $0.isVisible }
    }

    func ownWindowsChanged() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let windows = NSApp.windows
            let facts = windows.map {
                (isChat: $0 === self.panel, isTitled: $0.styleMask.contains(.titled) || $0 is QLPreviewPanel,
                 isVisible: $0.isVisible)
            }
            let covered = Self.coveredByOwnWindow(facts)
            guard covered != self.underOwnWindow else { return }
            self.underOwnWindow = covered
            self.applyPlace()
            // Down among normal windows, it goes behind the one that opened (a level change alone may leave it in front)
            if covered, self.panel.isVisible,
               let front = zip(windows, facts).first(where: { $0.0 === NSApp.keyWindow && !$0.1.isChat && $0.1.isTitled })
                ?? zip(windows, facts).first(where: { !$0.1.isChat && $0.1.isTitled && $0.1.isVisible }) {
                self.panel.order(.below, relativeTo: front.0.windowNumber)
            }
        }
    }

    /// Keep on top floats the window, except while FERB's own picker, alert, Quick Look or Settings shows, or after
    /// FERB handed something to another app (APP-748, APP-749).
    static func level(keepOnTop: Bool, picker: Bool, ownWindow: Bool, handedOff: Bool) -> NSWindow.Level {
        keepOnTop && !picker && !ownWindow && !handedOff ? .floating : .normal
    }

    /// In the overlay space the window floats above every ordinary window; while something must show above it, it
    /// steps back to the normal level in the ordinary window stack: in front of ordinary windows under another
    /// process's menu or popover, behind the app window the owner brought forward (`coverIfUnder`) or FERB's own window
    /// (`ownWindowsChanged`), and left where it is under the picker or after a hand-off, whose window comes forward
    /// by itself. Without the overlay space: Keep on top's level, as before (`level`).
    private func applyPlace() {
        guard let space, space.isAttached else {
            panel.level = Self.level(keepOnTop: controls.keepOnTop, picker: underPicker, ownWindow: underOwnWindow,
                                     handedOff: handedOff)
            return
        }
        if stepsBack {
            panel.level = .normal
            guard !steppedBack else { return }
            steppedBack = true
            space.stepBack(panel)
            Self.log.notice("Window steps back (\(self.reasons, privacy: .public))")
            if panel.isVisible && overlaysOver && coveredBy == nil && !underPicker && !underOwnWindow && !handedOff {
                panel.orderFrontRegardless()                  // menus and popovers sit above the normal level
            }
        } else {
            panel.level = .floating
            guard steppedBack else { return }
            steppedBack = false
            if panel.isVisible { panel.orderFrontRegardless() }
            space.stepForward(panel)
            Self.log.notice("Window comes back into the fixed space")
        }
    }

    var reasons: String {
        [underPicker ? "picker" : nil, underOwnWindow ? "FERB window" : nil, handedOff ? "hand-off" : nil,
         overlaysOver ? "menu or popover" : nil, coveredBy.map { "app \($0)" }].compactMap { $0 }.joined(separator: ", ")
    }

    static let log = Logger(subsystem: "com.ferb.FERB", category: "spaces")

    /// The displays changed: a window no longer reachable on any of them (its display unplugged) moves onto the
    /// pointer's display at its place there.
    func screensChanged() {
        guard panel.isVisible,
              !ChatWindowFrames.isReachable(panel.frame, on: screens().map(\.visible)) else { return }
        place()
    }

    private func place() {
        let all = screens()
        let point = pointer()
        guard let screen = all.first(where: { $0.visible.insetBy(dx: -1, dy: -40).contains(point) }) ?? all.first
        else { return }
        placing = true
        panel.setFrame(ChatWindowFrames.frame(display: screen.id, visible: screen.visible, in: defaults), display: false)
        placing = false
    }

    /// The display the window is on now (by the largest share of its frame).
    private var currentDisplay: UInt32? {
        let frame = panel.frame
        return screens().max { first, second in
            area(first.visible.intersection(frame)) < area(second.visible.intersection(frame))
        }?.id
    }

    private func area(_ rect: CGRect) -> CGFloat { rect.isNull ? 0 : rect.width * rect.height }

    private func remember() {
        guard !placing, panel.isVisible, let display = currentDisplay else { return }
        ChatWindowFrames.save(panel.frame, display: display, in: defaults)
    }

    func windowDidMove(_ notification: Notification) { remember() }
    func windowDidEndLiveResize(_ notification: Notification) { remember() }
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        onClose?()
        return false
    }
}

/// The window's content: a draggable header (title with its status dot, profile menu, close; Keep on top is a setting,
/// APP-758) over the one chat body (ChatBodyView), on black glass like the notch (APP-551).
struct ChatWindowView: View {
    @ObservedObject var model: NotchModel
    @ObservedObject var controls: ChatWindowControls
    static let headerHeight: CGFloat = 38

    var body: some View {
        VStack(spacing: 0) {
            header
            GeometryReader { proxy in
                Group {
                    if model.chatInWindow {
                        ChatBodyView(model: model, animates: false)
                            .environment(\.chatInWindowHost, true)
                            .padding(.horizontal, 25)
                            .padding(.top, 9)
                            .padding(.bottom, 17)
                    } else {
                        Color.clear
                    }
                }
                .frame(width: proxy.size.width, height: proxy.size.height, alignment: .top)
                .onAppear { report(proxy.size) }
                .onChange(of: proxy.size) { _, size in report(size) }
            }
        }
        .background { ChatWindowBackdrop().ignoresSafeArea() }
        .ignoresSafeArea()
        .environment(\.selectableTextGeneration, model.textRenderGeneration)
        .foregroundStyle(Palette.ink)
        .preferredColorScheme(.dark)
        .onDrop(of: [UTType.fileURL], delegate: ChatFileDrop(session: model.drops))
        .onExitCommand { controls.close() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("FERB window")
    }

    private func report(_ size: CGSize) {
        if model.windowChatSize != size { model.windowChatSize = size }
    }

    private var header: some View {
        HStack(spacing: 6) {
            ChatTitle(model: model)
            Spacer(minLength: 0)
            ProfileMenuButton(model: model).frame(width: 26, height: 26)
            Button(action: controls.close) {
                Image(systemName: "xmark").font(.system(size: 11, weight: .medium)).frame(width: 24, height: 26)
            }
            .buttonStyle(.plain).foregroundStyle(Palette.secondary)
            .help("Close · Esc or ⌘W").accessibilityLabel("Close FERB window")
        }
        .padding(.horizontal, 18)
        .frame(height: Self.headerHeight)
        .background(WindowDragArea())
    }
}

/// The header's empty space moves the window. The window isn't movable by the window server (APP-759), so the drag is
/// the app's own: the window follows the pointer from where the press began until it is released, and windowDidMove
/// remembers its place as before.
struct WindowDragArea: NSViewRepresentable {
    final class DragView: NSView {
        /// Where the pointer is on screen (tests stand in their own).
        var pointer: () -> NSPoint = { NSEvent.mouseLocation }
        private var start: (pointer: NSPoint, origin: NSPoint)?
        override var mouseDownCanMoveWindow: Bool { false }
        override func mouseDown(with event: NSEvent) {
            guard let window else { return }
            start = (pointer(), window.frame.origin)
        }
        override func mouseDragged(with event: NSEvent) {
            guard let window, let start else { return }
            let now = pointer()
            window.setFrameOrigin(NSPoint(x: start.origin.x + now.x - start.pointer.x,
                                          y: start.origin.y + now.y - start.pointer.y))
        }
        override func mouseUp(with event: NSEvent) { start = nil }
    }
    func makeNSView(context: Context) -> NSView { DragView() }
    func updateNSView(_ nsView: NSView, context: Context) {}
}

/// Frosted black glass: the desktop blurred behind, a black tint over it that is darkest at the top (APP-551), solid
/// black when Reduce Transparency is on.
private struct ChatWindowBackdrop: View {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    var body: some View {
        if reduceTransparency {
            Color.black
        } else {
            ZStack {
                VisualEffect()
                LinearGradient(stops: [.init(color: .black.opacity(0.94), location: 0),
                                       .init(color: .black.opacity(0.82), location: 0.35),
                                       .init(color: .black.opacity(0.7), location: 1)],
                               startPoint: .top, endPoint: .bottom)
            }
        }
    }

    private struct VisualEffect: NSViewRepresentable {
        func makeNSView(context: Context) -> NSVisualEffectView {
            let view = NSVisualEffectView()
            view.material = .hudWindow
            view.blendingMode = .behindWindow
            view.state = .active
            return view
        }
        func updateNSView(_ view: NSVisualEffectView, context: Context) {}
    }
}
