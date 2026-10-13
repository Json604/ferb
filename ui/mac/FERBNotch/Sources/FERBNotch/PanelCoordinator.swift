import AppKit
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
}

final class NotchLightPanel: NSPanel {
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
final class PanelCoordinator: NSObject, NSWindowDelegate, @preconcurrency QLPreviewPanelDataSource, QLPreviewPanelDelegate {
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
    private var settingsWindow: NSWindow?
    private var previewURL: URL?

    override init() {
        panel = NotchPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                           backing: .buffered, defer: false)
        super.init()
        configurePanel()
        model.onPresentationChange = { [weak self] animate in self?.present(animate: animate) }
        model.onAttentionArrival = { [weak self] in self?.hoverTask?.cancel() }
        model.onBackgroundChange = { [weak self] animate in self?.present(animate: animate, requestFocus: false) }
        model.onInteractionEnded = { [weak self] in
            guard let self, !self.pointerIsOverSurface else { return }
            self.pointerExited()
        }
        model.onInteraction = { [weak self] in self?.beginInteraction() }
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
        model.onStatusChange = { [weak self] in self?.updateTracerVisibility() }
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
        registerShortcut()
        shortcuts.onDoubleTap = { [weak self] in self?.model.toggle() }
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
        if let monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] _ in
            guard let self, self.model.expanded, !self.panel.frame.contains(NSEvent.mouseLocation) else { return }
            self.model.dismissAutomatically()
        }) { eventMonitors.append(monitor) }
        // Global monitors do not receive clicks in our Settings/Quick Look windows.
        if let monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown], handler: { [weak self] event in
            guard let self else { return event }
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
        tracerPanel.ignoresMouseEvents = true
        tracerPanel.isReleasedWhenClosed = false
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
        model.handleWidth = current.handleWidth
        model.hasHardwareNotch = current.hasNotch
        model.cutoutHeight = current.hasNotch ? screen.safeAreaInsets.top : 24
        // Native displays begin at the screen edge. Other displays get a small
        // equivalent tab immediately below their menu bar.
        let top = current.hasNotch ? screen.frame.maxY : current.topEdge
        cutoutFrame = CGRect(x: current.centerX - current.handleWidth / 2,
                             y: top - model.cutoutHeight,
                             width: current.handleWidth, height: model.cutoutHeight)
        tracerPanel.setFrame(frame(for: CGSize(width: model.handleWidth + 40,
                                               height: model.cutoutHeight + 24)), display: true)
        pointerIsInside = false
        present(animate: false, requestFocus: false)
    }

    @objc private func screenDidWake() {
        updateScreen()
        panel.orderFrontRegardless()
        updateTracerVisibility()
    }

    @objc private func activeSpaceChanged() {
        // Space changes are not display changes. Keep the same geometry, draft,
        // pin state, running spring and current text responder. In particular,
        // do not resignKey here: that discarded input focus on every swipe.
        hoverTask?.cancel()
        if !overlaySpace.isAttached {
            panel.orderFrontRegardless()
            updateTracerVisibility()
        }
        pointerIsInside = pointerIsOverSurface
    }

    private func present(animate: Bool, requestFocus: Bool = true) {
        guard geometry != nil else { return }
        let isOpening = model.bodyVisible && !presentedBodyVisible
        presentedBodyVisible = model.bodyVisible
        let targetFrame = frame(for: model.panelSize)
        let targetSize = CGSize(width: model.expanded ? model.bodyWidth : model.handleWidth,
                                height: model.cutoutHeight + (model.bodyVisible ? model.bodyHeight : 0))
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
        panel.orderFrontRegardless()
        updateTracerVisibility()
        if !model.bodyVisible {
            panel.makeFirstResponder(nil)
            panel.resignKey()
        } else if requestFocus && !model.voiceIsHeld && (isOpening || panel.isKeyWindow) {
            // The held voice chord leaves the current application's keyboard
            // focus intact until release. Background task updates also leave
            // focus with the user's active app, including after a Space change.
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
        if model.showsTraceGlow { tracerPanel.orderFrontRegardless() }
        else { tracerPanel.orderOut(nil) }
    }

    private func beginInteraction() {
        hoverTask?.cancel()
        if model.bodyVisible { panel.makeKey() }
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
        item.button?.image = NSImage(systemSymbolName: "waveform", accessibilityDescription: "FERB Notch")
        item.button?.toolTip = "FERB · Double-tap right Option to open / close · Hold right Command + Option for voice"
        let menu = NSMenu()
        addItem("Show FERB", action: #selector(showFromMenu), to: menu)
        addItem("Type a request", action: #selector(typeRequest), to: menu)
        addItem("Dictate a request", action: #selector(voicePreview), to: menu)
        addItem("Conversation history", action: #selector(recentExamples), to: menu)
        addItem("Settings…", action: #selector(showSettings), to: menu, key: ",")
        menu.addItem(.separator())
        let gestures = NSMenuItem(title: "Double-tap right ⌥ · Hold right ⌘ + ⌥ for voice", action: nil, keyEquivalent: "")
        gestures.isEnabled = false
        menu.addItem(gestures)
        let preview = NSMenuItem(title: "Connected to the local FERB daemon", action: nil, keyEquivalent: "")
        preview.isEnabled = false
        menu.addItem(preview)
        menu.addItem(.separator())
        addItem("Quit FERB Notch", action: #selector(quit), to: menu, key: "q")
        item.menu = menu
        statusItem = item
        if let appMenu = NSApp.mainMenu?.items.first?.submenu {
            let settings = NSMenuItem(title: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
            settings.target = self
            appMenu.insertItem(settings, at: 0)
            appMenu.insertItem(.separator(), at: 1)
        }
    }

    private func addItem(_ title: String, action: Selector, to menu: NSMenu, key: String = "") {
        let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
        item.target = self
        menu.addItem(item)
    }

    @objc func showFromMenu() { beginInteraction(); model.expand() }
    @objc private func typeRequest() { beginInteraction(); model.edit(keyboard: true) }
    @objc private func voicePreview() { beginInteraction(); model.startDictation() }
    @objc private func recentExamples() { beginInteraction(); model.history() }
    @objc private func quit() { NSApp.terminate(nil) }

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
        if model.expanded { model.collapse(keyboard: true); return }
        beginInteraction()
        if model.session.phase == .ready || model.session.phase == .draft { model.edit(keyboard: true) }
        else { model.expand(keyboard: true); panel.makeKey() }
    }

    @objc private func showSettings() {
        if !model.isPinned { model.collapse() }
        if settingsWindow == nil {
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 410, height: 350),
                                  styleMask: [.titled, .closable],
                                  backing: .buffered, defer: false)
            window.title = "FERB Settings"
            window.isReleasedWhenClosed = false
            window.contentView = NSHostingView(rootView: FeedbackSettingsView(feedback: model.feedback))
            window.center()
            settingsWindow = window
        }
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
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
        model.disconnectDaemon()
        shortcuts.stop()
        model.cancelPinnedFold()
        presentation.stop()
        panel.orderOut(nil)
        tracerPanel.orderOut(nil)
        overlaySpace.stop()
        presentationRevision = UUID()
        hoverTask?.cancel()
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let hotKeyHandler { RemoveEventHandler(hotKeyHandler) }
        eventMonitors.forEach(NSEvent.removeMonitor)
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
        if let statusItem { NSStatusBar.system.removeStatusItem(statusItem) }
    }
}
