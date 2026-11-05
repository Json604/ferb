import AppKit
import SwiftUI
import UniformTypeIdentifiers
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
    /// (the input method takes it). An open menu runs its own event loop, so its Esc never arrives here.
    override func sendEvent(_ event: NSEvent) {
        if event.type == .keyDown, Self.isPlainEscape(event), !composingText {
            onClose?()
            return
        }
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

/// The window's own controls, observed by its header: Keep on top (D20; named so it isn't taken for the notch's
/// removed pin, APP-743) and close.
@MainActor
final class ChatWindowControls: ObservableObject {
    @Published var keepOnTop: Bool
    var toggleKeepOnTop: () -> Void = {}
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
        controls.toggleKeepOnTop = { [weak self] in
            guard let self else { return }
            self.defaults.set(!self.controls.keepOnTop, forKey: ChatHost.keepOnTopKey)
            self.settingsChanged()
        }
        let hosting = NSHostingView(rootView: ChatWindowView(model: model, controls: controls))
        hosting.sizingOptions = []
        panel.contentView = hosting
        applyLevel()
    }

    var isShown: Bool { panel.isVisible }

    /// Shows the window: placed on the pointer's display at its saved place when it wasn't showing, brought in front
    /// of other windows without taking keyboard focus (D22), and made key only for `focus` (the owner opened it).
    func show(focus: Bool) {
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

    private func applyLevel() {
        panel.level = controls.keepOnTop && !underPicker ? .floating : .normal
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

/// The window's content: a draggable header (title with its status dot, profile menu, Keep on top, close) over the
/// one chat body (ChatBodyView), on black glass like the notch (APP-551).
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
            Button(action: controls.toggleKeepOnTop) {
                Image(systemName: controls.keepOnTop ? "pin.fill" : "pin")
                    .font(.system(size: 11)).frame(width: 24, height: 26)
                    .foregroundStyle(controls.keepOnTop ? Palette.ink : Palette.secondary)
            }
            .buttonStyle(.plain)
            .help(controls.keepOnTop ? "Keep on top: on" : "Keep on top: off")
            .accessibilityLabel("Keep on top")
            .accessibilityValue(controls.keepOnTop ? "On" : "Off")
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

/// The header's empty space moves the window (the window has no title bar of its own to drag).
private struct WindowDragArea: NSViewRepresentable {
    final class DragView: NSView {
        override func mouseDown(with event: NSEvent) { window?.performDrag(with: event) }
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
