import AppKit
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
    private var observers: [NSObjectProtocol] = []

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
        applyLevel()
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

    deinit { observers.forEach(NotificationCenter.default.removeObserver) }

    var isShown: Bool { panel.isVisible }

    /// Shows the window: placed on the pointer's display at its saved place when it wasn't showing, brought in front
    /// of other windows without taking keyboard focus (D22), and made key only for `focus` (the owner opened it).
    func show(focus: Bool) {
        if handedOff { ownerReturned() }
        if !panel.isVisible { place() }
        panel.orderFrontRegardless()
        if focus { panel.makeKey() }
    }

    func hide() {
        guard panel.isVisible else { return }
        panel.orderOut(nil)
    }

    /// Keep on top (on: floating; off: among other windows) read again, e.g. after Settings changed it.
    func settingsChanged() {
        controls.keepOnTop = ChatHost.keepsOnTop(defaults)
        applyLevel()
    }

    /// FERB's file picker opened or closed (it must show above the window).
    func filePicker(open: Bool) {
        underPicker = open
        applyLevel()
    }

    /// FERB opened something in another app: what opened shows above the window.
    func handOff() {
        handedOff = true
        applyLevel()
    }

    private func ownerReturned() {
        handedOff = false
        applyLevel()
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
            self.applyLevel()
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

    private func applyLevel() {
        panel.level = Self.level(keepOnTop: controls.keepOnTop, picker: underPicker, ownWindow: underOwnWindow,
                                 handedOff: handedOff)
    }

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
