import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
import NotchCore

/// Where FERB opens: Notch | Window (owner's item 8, D19–D23). Driven through the real PanelCoordinator with a scratch
/// defaults suite, a scripted daemon and the pointer placed by the test.
@MainActor
final class WindowModeTests: XCTestCase {
    private struct Harness {
        let defaults: UserDefaults
        let model: NotchModel
        let coordinator: PanelCoordinator
        let notchPanel: NotchPanel
    }

    private var pointer = CGPoint.zero

    private func harness(window: Bool = false, hidePill: Bool = false) throws -> Harness {
        _ = NSApplication.shared
        guard NSScreen.screens.first != nil else { throw XCTSkip("Needs a display") }
        let defaults = scratchDefaults()
        NotchDocking.save(true, in: defaults)
        if window { defaults.set(ChatHost.window.rawValue, forKey: ChatHost.defaultsKey) }
        if hidePill { defaults.set(true, forKey: ChatHost.hidePillKey) }
        let model = NotchModel(rpc: ScriptedRPC(), preferences: defaults)
        let coordinator = PanelCoordinator(model: model, defaults: defaults)
        coordinator.updateScreen()
        coordinator.hoverWait = { _ in }
        coordinator.pointerLocation = { [unowned self] in self.pointer }
        let panel = try XCTUnwrap(NSApp.windows.first { $0.delegate === coordinator } as? NotchPanel)
        addTeardownBlock { coordinator.tearDown() }
        return Harness(defaults: defaults, model: model, coordinator: coordinator, notchPanel: panel)
    }

    private func setMode(_ h: Harness, _ host: ChatHost, hidePill: Bool? = nil) {
        h.defaults.set(host.rawValue, forKey: ChatHost.defaultsKey)
        if let hidePill { h.defaults.set(hidePill, forKey: ChatHost.hidePillKey) }
        h.coordinator.applyChatHost()
    }

    private func window(_ h: Harness) throws -> ChatWindowController { try XCTUnwrap(h.coordinator.chatWindow) }

    private func key(_ characters: String, flags: NSEvent.ModifierFlags = [], code: UInt16 = 0,
                     in window: NSWindow) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                                       windowNumber: window.windowNumber, context: nil, characters: characters,
                                       charactersIgnoringModifiers: characters, isARepeat: false, keyCode: code))
    }

    private func scratchFile(_ name: String) throws -> URL {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FERBWindowMode-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: folder) }
        let url = folder.appendingPathComponent(name)
        try Data("test".utf8).write(to: url)
        return url
    }

    // decision: APP-745, APP-550
    func testInNotchModeNoWindowIsEverCreatedAndTheNotchOpensAsBefore() throws {
        let h = try harness()
        h.coordinator.toggleByShortcut()
        XCTAssertTrue(h.model.bodyVisible)
        XCTAssertTrue(h.notchPanel.isVisible)
        h.coordinator.toggleByShortcut()
        XCTAssertFalse(h.model.expanded)
        h.coordinator.showFromMenu()
        h.model.announceAttention("test:notch-arrival")
        h.model.dropFiles([try scratchFile("a.txt")])
        XCTAssertFalse(h.model.chatInWindow)
        XCTAssertNil(h.coordinator.chatWindow)
        XCTAssertFalse(NSApp.windows.contains { $0.identifier?.rawValue == "FERBChatWindow" && $0.isVisible })
    }

    // decision: APP-745, APP-746
    func testWindowModeOpensByShortcutMenuReopenDropAndPressAndHidesByCloseCommandWAndEscape() throws {
        let h = try harness(window: true)
        XCTAssertNil(h.coordinator.chatWindow, "nothing is created before the window first opens")
        // The shortcut (double-tap and the owner's combination share toggleByShortcut).
        h.coordinator.toggleByShortcut()
        let w = try window(h)
        XCTAssertTrue(h.model.expanded)
        XCTAssertTrue(w.isShown)
        XCTAssertFalse(h.notchPanel.isVisible, "the notch shows no chat in window mode")
        XCTAssertTrue(h.model.staysOpenOnHoverOut)
        // Nothing dismisses it automatically: a click in another app, hover-out, Settings.
        h.coordinator.ownerClickedInAnotherApp()
        h.model.dismissAutomatically()
        XCTAssertTrue(w.isShown)
        // Close button.
        w.controls.close()
        XCTAssertFalse(w.isShown)
        XCTAssertFalse(h.model.expanded)
        // The menu bar's Open FERB and opening FERB.app again (applicationShouldHandleReopen → showFromMenu).
        h.coordinator.perform(.openFERB)
        XCTAssertTrue(w.isShown)
        XCTAssertTrue(w.panel.performKeyEquivalent(with: try key("w", flags: .command, code: 13, in: w.panel)))
        XCTAssertFalse(w.isShown, "⌘W hides it")
        h.coordinator.showFromMenu()
        XCTAssertTrue(w.isShown)
        w.panel.sendEvent(try key("\u{1b}", code: 53, in: w.panel))
        XCTAssertFalse(w.isShown, "Esc hides it")
        // A file drop opens it with the files in the composer, sending nothing.
        let file = try scratchFile("brief.txt")
        XCTAssertTrue(h.model.dropFiles([file]))
        XCTAssertTrue(w.isShown)
        XCTAssertEqual(h.model.session.attachments, [file])
        XCTAssertTrue(h.model.conversation.turns.isEmpty)
        w.panel.cancelOperation(nil)
        XCTAssertFalse(w.isShown)
        // A press on the notch (the pill shown) opens it too.
        let control = try XCTUnwrap(h.coordinator.indicatorControl)
        control.onClick?()
        XCTAssertTrue(w.isShown)
        // The shortcut hides it while it has keyboard focus; otherwise it brings it forward.
        if w.panel.isKeyWindow {
            h.coordinator.toggleByShortcut()
            XCTAssertFalse(w.isShown)
        }
    }

    // decision: APP-746
    func testHoverNeverOpensTheWindow() async throws {
        let h = try harness(window: true)
        h.model.setHoverEnabled(true)
        let control = try XCTUnwrap(h.coordinator.indicatorControl)
        let frame = try XCTUnwrap(control.window).frame
        pointer = CGPoint(x: frame.midX, y: frame.maxY - 4)
        h.coordinator.updatePointer()
        await h.coordinator.hoverTask?.value
        XCTAssertFalse(h.model.expanded)
        XCTAssertNil(h.coordinator.chatWindow)
    }

    // decision: APP-745, APP-748, APP-577
    func testTheNotchAndTheWindowShowTheSameChatAndSwitchingKeepsTheDraftAndAttachments() throws {
        let h = try harness()
        let file = try scratchFile("notes.txt")
        h.model.toggle(byPress: true)
        h.model.session.draft = "Draft typed in the notch"
        h.model.session.attach([file])
        setMode(h, .window)
        let w = try window(h)
        XCTAssertTrue(h.model.chatInWindow)
        XCTAssertTrue(w.isShown, "the open chat moved to the window")
        XCTAssertFalse(h.notchPanel.isVisible)
        XCTAssertFalse(w.panel.isKeyWindow, "moving it never takes keyboard focus")
        XCTAssertEqual(h.model.session.draft, "Draft typed in the notch")
        XCTAssertEqual(h.model.session.attachments, [file])
        let hosting = try XCTUnwrap(w.panel.contentView as? NSHostingView<ChatWindowView>)
        XCTAssertTrue(hosting.rootView.model === h.model, "one conversation: the window shows the notch's own model")
        // Sent in the window, there in the notch.
        h.model.session.attachments.forEach { h.model.removeAttachment($0) }
        h.model.session.draft = "Sent from the window"
        h.model.startWork()
        XCTAssertEqual(h.model.conversation.turns.last?.request, "Sent from the window")
        h.model.session.draft = "Next draft"
        setMode(h, .notch)
        XCTAssertFalse(h.model.chatInWindow)
        XCTAssertFalse(w.isShown)
        XCTAssertTrue(h.notchPanel.isVisible, "the open chat moved back to the notch")
        XCTAssertEqual(h.model.conversation.turns.last?.request, "Sent from the window")
        XCTAssertEqual(h.model.session.draft, "Next draft")
    }

    // decision: APP-745, APP-301, APP-571, APP-622
    func testQuestionsAnswersAndRemindersShowTheWindowWithoutKeyboardFocus() throws {
        let h = try harness(window: true)
        h.model.announceAttention("test:question")
        let w = try window(h)
        XCTAssertTrue(w.isShown)
        XCTAssertFalse(w.panel.isKeyWindow)
        w.controls.close()
        h.model.presentTerminalAnswer()
        XCTAssertTrue(w.isShown)
        XCTAssertFalse(w.panel.isKeyWindow)
        w.controls.close()
        let card = try XCTUnwrap(ResultCard(["type": "reminder", "title": "Water"]))
        h.model.presentReminder(card: card, text: "Drink some water.")
        XCTAssertTrue(w.isShown)
        XCTAssertFalse(w.panel.isKeyWindow)
        XCTAssertNil(h.model.compactPlanText, "no compact notch arrival in window mode")
        XCTAssertFalse(h.notchPanel.isVisible)
    }

    // decision: APP-747, APP-564
    func testHidingThePillHidesTheHandleEdgeRowAndPillAndTheMenuBarIconCarriesTheStatus() throws {
        let h = try harness(window: true)
        XCTAssertTrue(h.coordinator.indicatorPanel.isVisible || h.coordinator.edgeRowPanel.isVisible
                      || h.model.isNotchDocked)
        XCTAssertNil(h.coordinator.statusTint, "the pill shows FERB's status itself")
        setMode(h, .window, hidePill: true)
        XCTAssertTrue(h.coordinator.pillHidden)
        XCTAssertFalse(h.coordinator.indicatorPanel.isVisible)
        XCTAssertFalse(h.coordinator.edgeRowPanel.isVisible)
        h.coordinator.updateScreen()                          // a layout asks for them again: they stay out
        XCTAssertFalse(h.coordinator.indicatorPanel.isVisible)
        XCTAssertFalse(h.coordinator.edgeRowPanel.isVisible)
        XCTAssertEqual(h.coordinator.statusTint, StatusTint.of(h.model.indicatorPhase, pillHidden: true))
        // The shortcut and the menu still bring FERB back.
        h.coordinator.showFromMenu()
        XCTAssertTrue(try window(h).isShown)
        // Back to the notch: Hide the pill applies only in window mode.
        setMode(h, .notch)
        XCTAssertFalse(h.coordinator.pillHidden)
        XCTAssertTrue(h.coordinator.indicatorPanel.isVisible || h.coordinator.edgeRowPanel.isVisible
                      || h.notchPanel.isVisible)
    }

    // decision: APP-564, APP-747
    func testTheMenuBarTintIsThePillsStatusColourOnlyWhileThePillIsHidden() {
        XCTAssertEqual(StatusTint.of(.working, pillHidden: true), .amber)
        XCTAssertEqual(StatusTint.of(.attention, pillHidden: true), .red)
        XCTAssertEqual(StatusTint.of(.result, pillHidden: true), .green)
        XCTAssertNil(StatusTint.of(.ready, pillHidden: true))
        XCTAssertNil(StatusTint.of(.attention, pillHidden: false))
        let defaults = scratchDefaults()
        defaults.set(true, forKey: ChatHost.hidePillKey)
        XCTAssertFalse(ChatHost.hidesPill(defaults), "Hide the pill applies only in window mode")
        defaults.set(ChatHost.window.rawValue, forKey: ChatHost.defaultsKey)
        XCTAssertTrue(ChatHost.hidesPill(defaults))
    }

    // decision: APP-745, APP-642
    func testTheTourAlwaysRunsInTheNotchAndWindowModeReturnsAfterIt() throws {
        let h = try harness(window: true)
        h.coordinator.toggleByShortcut()
        let w = try window(h)
        XCTAssertTrue(w.isShown)
        h.model.onIntroAction = { _ in }                         // the tour (or Replay intro) starts
        XCTAssertFalse(h.model.chatInWindow)
        XCTAssertFalse(w.isShown)
        XCTAssertTrue(h.notchPanel.isVisible, "the open chat is in the notch for the tour")
        h.coordinator.toggleByShortcut()
        XCTAssertFalse(w.isShown, "the shortcut works the notch during the tour")
        h.model.onIntroAction = nil                              // the tour ended
        XCTAssertTrue(h.model.chatInWindow)
        h.coordinator.toggleByShortcut()
        XCTAssertTrue(w.isShown)
    }

    // decision: APP-745, APP-746
    func testTheWindowsPlaceIsKeptPerDisplayAndAnUnpluggedDisplayMovesItOntoAVisibleOne() throws {
        let h = try harness(window: true)
        // Two stand-in displays inside the real one (AppKit keeps a titled window on a real screen).
        let real = try XCTUnwrap(NSScreen.screens.first).visibleFrame
        guard real.width >= 1000, real.height >= 700 else { throw XCTSkip("Needs a larger display") }
        let left = CGRect(x: real.minX, y: real.minY, width: real.width / 2, height: real.height)
        let right = CGRect(x: real.midX, y: real.minY, width: real.width / 2, height: real.height)
        h.coordinator.toggleByShortcut()
        let w = try window(h)
        w.controls.close()
        w.screens = { [(id: UInt32(1), visible: left), (id: UInt32(2), visible: right)] }
        pointer = CGPoint(x: right.midX, y: right.midY)
        w.pointer = { [unowned self] in self.pointer }
        h.coordinator.toggleByShortcut()
        XCTAssertEqual(w.panel.frame, ChatWindowFrames.standard(in: right))
        let moved = CGRect(x: right.minX + 40, y: right.minY + 30, width: 480, height: 600)
        w.panel.setFrame(moved, display: false)
        w.windowDidMove(Notification(name: NSWindow.didMoveNotification))
        XCTAssertEqual(ChatWindowFrames.saved(display: 2, in: h.defaults), moved)
        w.controls.close()
        pointer = CGPoint(x: left.midX, y: left.midY)
        h.coordinator.toggleByShortcut()
        XCTAssertEqual(w.panel.frame, ChatWindowFrames.standard(in: left), "another display, its own place")
        w.controls.close()
        pointer = CGPoint(x: right.midX, y: right.midY)
        h.coordinator.toggleByShortcut()
        XCTAssertEqual(w.panel.frame, moved, "back on the right display, where the owner left it")
        // The right display is unplugged while the window shows there.
        w.screens = { [(id: UInt32(1), visible: left)] }
        w.screensChanged()
        XCTAssertTrue(left.contains(w.panel.frame))
    }

    // decision: APP-745
    func testFramesFitTheVisibleAreaAndUnreachableFramesAreDetected() {
        let visible = CGRect(x: 0, y: 25, width: 1440, height: 850)
        let standard = ChatWindowFrames.standard(in: visible)
        XCTAssertTrue(visible.contains(standard))
        XCTAssertEqual(standard.size, ChatWindowFrames.standardSize)
        let huge = ChatWindowFrames.fitted(CGRect(x: -500, y: -500, width: 5000, height: 5000), in: visible)
        XCTAssertEqual(huge, visible)
        let tiny = ChatWindowFrames.fitted(CGRect(x: 100, y: 100, width: 10, height: 10), in: visible)
        XCTAssertEqual(tiny.size, ChatWindowFrames.minimumSize)
        XCTAssertFalse(ChatWindowFrames.isReachable(CGRect(x: 2000, y: 200, width: 400, height: 500), on: [visible]))
        XCTAssertTrue(ChatWindowFrames.isReachable(CGRect(x: 1300, y: 200, width: 400, height: 500), on: [visible]))
    }

    // decision: APP-745, APP-748
    func testKeepOnTopSwitchesTheLevelAndFERBsFilePickerShowsAboveTheWindow() throws {
        let h = try harness(window: true)
        h.coordinator.toggleByShortcut()
        let w = try window(h)
        XCTAssertEqual(w.panel.level, .floating, "floats on top by default")
        h.defaults.set(false, forKey: ChatHost.keepOnTopKey)
        h.coordinator.applyChatHost()
        XCTAssertEqual(w.panel.level, .normal)
        XCTAssertFalse(w.controls.keepOnTop)
        h.defaults.set(true, forKey: ChatHost.keepOnTopKey)
        h.coordinator.applyChatHost()
        XCTAssertEqual(w.panel.level, .floating)
        XCTAssertEqual(h.defaults.object(forKey: ChatHost.keepOnTopKey) as? Bool, true)
        h.model.setFilePickerOpen(true)
        XCTAssertEqual(w.panel.level, .normal, "the picker shows above the window")
        XCTAssertTrue(w.isShown)
        h.model.setFilePickerOpen(false)
        XCTAssertEqual(w.panel.level, .floating)
    }

    // decision: APP-749
    func testFERBsOwnWindowsAndWhatItOpensElsewhereShowAboveAKeptOnTopWindow() throws {
        let h = try harness(window: true)
        h.coordinator.toggleByShortcut()
        let w = try window(h)
        XCTAssertEqual(w.panel.level, .floating)
        // Settings (a titled FERB window) opens: the window steps down and goes behind it, and floats again after
        let settings = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
                                styleMask: [.titled, .closable], backing: .buffered, defer: false)
        settings.isReleasedWhenClosed = false
        addTeardownBlock { settings.orderOut(nil) }
        settings.orderFrontRegardless()
        w.ownWindowsChanged()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(w.panel.level, .normal, "FERB's own window shows above the sticky window")
        settings.orderOut(nil)
        w.ownWindowsChanged()
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(w.panel.level, .floating)
        // A file shown in Finder (any hand-off): down until the owner comes back to the window
        HandOff.announce()
        XCTAssertEqual(w.panel.level, .normal)
        h.coordinator.toggleByShortcut()                 // hidden
        h.coordinator.toggleByShortcut()                 // and opened again by the owner
        XCTAssertEqual(w.panel.level, .floating)
        // The notch's own borderless panels never count; Keep on top off stays normal whatever shows
        XCTAssertFalse(ChatWindowController.coveredByOwnWindow([(isChat: true, isTitled: true, isVisible: true),
                                                                 (isChat: false, isTitled: false, isVisible: true)]))
        XCTAssertEqual(ChatWindowController.level(keepOnTop: false, picker: false, ownWindow: false, handedOff: false),
                       .normal)
    }

    // decision: APP-745, APP-748, APP-600
    func testTheWindowJoinsEverySpaceStaysOutOfCommandTabAndTheDockAndNeverActivatesFERB() throws {
        let h = try harness(window: true)
        h.coordinator.toggleByShortcut()
        let w = try window(h)
        let behavior = w.panel.collectionBehavior
        XCTAssertTrue(behavior.contains(.canJoinAllSpaces))
        XCTAssertTrue(behavior.contains(.fullScreenAuxiliary))
        XCTAssertTrue(behavior.contains(.ignoresCycle))
        XCTAssertTrue(w.panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertTrue(w.panel.styleMask.contains(.resizable))
        XCTAssertFalse(DockPresence.wantsDock([DockPresence.DockWindow(w.panel)]))
        XCTAssertEqual(w.panel.minSize, ChatWindowFrames.minimumSize)
    }

    // decision: APP-728, APP-745
    func testTheGuideCarriesWhereFERBOpensKeepOnTopAndHideThePill() {
        let defaults = scratchDefaults()
        func value(_ name: String) -> String? {
            (AppGuide.payload(defaults)["app_settings"] as? [[String: String]])?.first { $0["name"] == name }?["value"]
        }
        XCTAssertEqual(value(AppSettings.whereFERBOpens.name), "Notch")
        XCTAssertEqual(value(AppSettings.keepOnTop.name), "on")
        XCTAssertEqual(value(AppSettings.hidePill.name), "off")
        defaults.set(ChatHost.window.rawValue, forKey: ChatHost.defaultsKey)
        defaults.set(true, forKey: ChatHost.hidePillKey)
        defaults.set(false, forKey: ChatHost.keepOnTopKey)
        defaults.set(false, forKey: AppSettings.doubleTap.key)
        XCTAssertEqual(value(AppSettings.whereFERBOpens.name), "Window")
        XCTAssertEqual(value(AppSettings.keepOnTop.name), "off")
        XCTAssertEqual(value(AppSettings.hidePill.name), "on")
        XCTAssertEqual(AppGuide.openKeys(defaults), "No keyboard shortcut: FERB's menu bar icon → Open FERB")
        ChatWindowFrames.save(CGRect(x: 0, y: 0, width: 400, height: 500), display: 7, in: defaults)
        XCTAssertEqual(value(AppSettings.windowPlace.name), "saved on 1 display")
    }

    // decision: APP-748
    func testTheTranscriptPlaceGoesWithTheChat() {
        var anchors: [String: CGRect] = [:]
        let first = UUID(), second = UUID()
        anchors[TranscriptTarget.entry(first)] = CGRect(x: 0, y: -400, width: 300, height: 200)
        anchors[TranscriptTarget.entry(second)] = CGRect(x: 0, y: -50, width: 300, height: 200)
        anchors[TranscriptTarget.question("q")] = CGRect(x: 0, y: -100, width: 300, height: 400)
        XCTAssertEqual(TranscriptResume.at(anchors, following: false).topEntry, TranscriptTarget.entry(second))
        XCTAssertNil(TranscriptResume.at(anchors, following: true).topEntry)
    }
}
